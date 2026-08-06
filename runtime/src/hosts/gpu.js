/**
 * WebGPU host for `system.gpu`. Buffers/textures/surfaces are tracked by integer id; kernels come
 * from the sibling `.wgsl` + `abi.gpu.kernels` metadata attached via `attachGpuAbi`.
 */
function makeGpuHost(getInstance) {
  const buffers = new Map(); // id -> { gpuBuffer, nbytes, cpu }
  const shaders = new Map();
  const textures = new Map(); // id -> { texture, width, height, cpu }
  const surfaces = new Map(); // id -> { canvas, context, width, height, configured }
  const pipelineCache = new Map();
  let nextId = 1;
  let devicePromise = null;
  let device = null;
  let gpuAbi = null;
  let wgslSource = null;
  let blitPipeline = null;
  let blitSampler = null;
  let blitBindLayout = null;

  const ERR_UNAVAILABLE = 1;
  const ERR_TIMEOUT = 2;
  const ERR_VALIDATION = 3;
  const ERR_OTHER = 4;

  function classifyErr(err) {
    const msg = String(err && err.message ? err.message : err);
    if (/not available|no WebGPU|no WebGPU adapter/i.test(msg)) return ERR_UNAVAILABLE;
    if (/timed out|timeout/i.test(msg)) return ERR_TIMEOUT;
    if (/WGSL|validation|compile/i.test(msg)) return ERR_VALIDATION;
    return ERR_OTHER;
  }

  async function ensureDevice() {
    if (device) return device;
    if (!devicePromise) {
      devicePromise = (async () => {
        if (!globalThis.navigator?.gpu) {
          throw new Error("WebGPU is not available in this environment");
        }
        const adapter = await Promise.race([
          navigator.gpu.requestAdapter(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error("WebGPU requestAdapter timed out")), 8000),
          ),
        ]);
        if (!adapter) throw new Error("no WebGPU adapter");
        device = await adapter.requestDevice();
        return device;
      })().catch((err) => {
        devicePromise = null;
        throw err;
      });
    }
    return devicePromise;
  }

  function attachFromAbi(abi, sourceHint) {
    gpuAbi = abi && abi.gpu ? abi.gpu : null;
    if (gpuAbi && typeof sourceHint === "string") {
      wgslSource = sourceHint.replace(/\.wasm$/, ".wgsl").replace(/\.abi\.json$/, ".wgsl");
    }
  }

  function toU8(data) {
    return data instanceof Uint8Array ? data : Uint8Array.from(data || []);
  }

  async function ensureBlit(dev) {
    if (blitPipeline) return;
    const code = `
struct VSOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f, };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> VSOut {
  var positions = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  var uvs = array<vec2f, 3>(vec2f(0.0, 1.0), vec2f(2.0, 1.0), vec2f(0.0, -1.0));
  var o: VSOut;
  o.pos = vec4f(positions[vi], 0.0, 1.0);
  o.uv = uvs[vi];
  return o;
}
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;
@fragment fn fs(i: VSOut) -> @location(0) vec4f {
  return textureSample(tex, samp, i.uv);
}`;
    const module = dev.createShaderModule({ code });
    blitBindLayout = dev.createBindGroupLayout({
      entries: [
        { binding: 0, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "filtering" } },
        { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: "float" } },
      ],
    });
    blitPipeline = await dev.createRenderPipelineAsync({
      layout: dev.createPipelineLayout({ bindGroupLayouts: [blitBindLayout] }),
      vertex: { module, entryPoint: "vs" },
      fragment: {
        module,
        entryPoint: "fs",
        targets: [{ format: navigator.gpu.getPreferredCanvasFormat() }],
      },
      primitive: { topology: "triangle-list" },
    });
    blitSampler = dev.createSampler({ magFilter: "linear", minFilter: "linear" });
  }

  let loadWgslText = async (url) => {
    if (!url) throw new Error("no .wgsl URL; compile with Dream to emit sibling .wgsl");
    if (typeof fetch === "function") {
      const res = await fetch(url);
      if (!res.ok) throw new Error(`failed to fetch ${url}`);
      return await res.text();
    }
    throw new Error("fetch unavailable for .wgsl");
  };

  async function syncBufferToCpu(dev, b) {
    if (!b.gpuBuffer) return;
    const staging = dev.createBuffer({
      size: b.nbytes,
      usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
    });
    const encoder = dev.createCommandEncoder();
    encoder.copyBufferToBuffer(b.gpuBuffer, 0, staging, 0, b.nbytes);
    dev.queue.submit([encoder.finish()]);
    await staging.mapAsync(GPUMapMode.READ);
    const copy = staging.getMappedRange().slice(0);
    staging.unmap();
    staging.destroy();
    b.cpu = new Uint8Array(copy);
  }

  const host = {
    __attachGpuAbi: attachFromAbi,

    gpuIsAvailable: () => !!(globalThis.navigator && globalThis.navigator.gpu),
    gpuReady: () => device != null,
    gpuTryInit: async () => {
      try {
        await ensureDevice();
        return 0;
      } catch (e) {
        console.error("Dream Gpu.try_init:", e);
        return classifyErr(e);
      }
    },
    gpuFrame: () =>
      new Promise((resolve) => {
        if (typeof requestAnimationFrame === "function") {
          requestAnimationFrame(() => resolve());
        } else {
          setTimeout(resolve, 16);
        }
      }),
    gpuTimestamp: async () => {
      if (typeof performance !== "undefined" && performance.now) {
        return BigInt(Math.floor(performance.now() * 1e6));
      }
      return BigInt(Date.now()) * 1000000n;
    },

    gpuBufferAllocBytes: (n) => {
      const id = nextId++;
      buffers.set(id, { gpuBuffer: null, nbytes: Math.max(0, n | 0), cpu: null });
      return id;
    },
    gpuBufferWriteBytes: (id, data) => {
      const b = buffers.get(id);
      if (!b) throw new Error(`unknown GpuBuffer ${id}`);
      const arr = toU8(data);
      b.cpu = arr;
      b.nbytes = arr.byteLength;
      b.gpuBuffer = null;
    },
    gpuBufferWriteBytesAt: (id, byteOffset, data) => {
      const b = buffers.get(id);
      if (!b) throw new Error(`unknown GpuBuffer ${id}`);
      const arr = toU8(data);
      const off = Math.max(0, byteOffset | 0);
      if (!(b.cpu instanceof Uint8Array) || b.cpu.byteLength < b.nbytes) {
        b.cpu = new Uint8Array(Math.max(b.nbytes, off + arr.byteLength));
      }
      if (off + arr.byteLength > b.cpu.byteLength) {
        const grown = new Uint8Array(off + arr.byteLength);
        grown.set(b.cpu);
        b.cpu = grown;
      }
      b.cpu.set(arr, off);
      b.nbytes = Math.max(b.nbytes, off + arr.byteLength);
      b.gpuBuffer = null;
    },
    gpuBufferReadBytes: async (id, n) => host.gpuBufferReadBytesAt(id, 0, n),
    gpuBufferReadBytesAt: async (id, byteOffset, n) => {
      const b = buffers.get(id);
      if (!b) throw new Error(`unknown GpuBuffer ${id}`);
      const nbytes = Math.max(0, n | 0);
      const off = Math.max(0, byteOffset | 0);
      if (b.gpuBuffer) {
        const dev = await ensureDevice();
        await syncBufferToCpu(dev, b);
      }
      if (!(b.cpu instanceof Uint8Array) && !b.cpu) {
        return Array(nbytes).fill(0);
      }
      const src = b.cpu instanceof Uint8Array ? b.cpu : new Uint8Array(b.cpu.buffer || []);
      const slice = src.slice(off, off + nbytes);
      if (slice.length >= nbytes) return Array.from(slice);
      const out = Array(nbytes).fill(0);
      for (let i = 0; i < slice.length; i++) out[i] = slice[i];
      return out;
    },

    gpuDispatch: async (kernel, bufferIds, ex, ey, ez, uniforms) => {
      try {
        const dev = await ensureDevice();
        const meta = (gpuAbi && gpuAbi.kernels || []).find((k) => k.name === kernel);
        if (!meta) throw new Error(`unknown @compute kernel '${kernel}'`);
        const code = (typeof meta.source === "string" && meta.source.length > 0)
          ? meta.source
          : await loadWgslText(wgslSource);
        let pipe = pipelineCache.get(kernel);
        if (!pipe) {
          const module = dev.createShaderModule({ code });
          if (typeof module.getCompilationInfo === "function") {
            const info = await module.getCompilationInfo();
            const errs = (info.messages || []).filter((m) => m.type === "error");
            if (errs.length) {
              throw new Error(`WGSL compile error in kernel '${kernel}':\n` +
                errs.map((m) => `${m.message} @${m.lineNum}:${m.linePos}`).join("\n"));
            }
          }
          const entries = (meta.bindings || []).map((b) => ({
            binding: b.binding,
            visibility: GPUShaderStage.COMPUTE,
            buffer: {
              type: b.kind === "uniform"
                ? "uniform"
                : (b.read_write ? "storage" : "read-only-storage"),
            },
          }));
          const seen = new Set();
          const unique = [];
          for (const e of entries) {
            if (seen.has(e.binding)) continue;
            seen.add(e.binding);
            unique.push(e);
          }
          const layout = dev.createBindGroupLayout({ entries: unique });
          const pipeline = await dev.createComputePipelineAsync({
            layout: dev.createPipelineLayout({ bindGroupLayouts: [layout] }),
            compute: { module, entryPoint: meta.entry },
          });
          pipe = { pipeline, layout, meta };
          pipelineCache.set(kernel, pipe);
        }
        const ids = bufferIds || [];
        const resources = [];
        const usedBindings = new Set();
        let storageIdx = 0;
        const extra = toU8(uniforms);
        for (const bind of meta.bindings || []) {
          if (usedBindings.has(bind.binding)) continue;
          usedBindings.add(bind.binding);
          if (bind.kind === "uniform") {
            const ubuf = dev.createBuffer({
              size: 256,
              usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
            });
            const bytes = new Uint8Array(256);
            const i32 = new Int32Array(bytes.buffer);
            i32[0] = ex | 0;
            i32[1] = ey | 0;
            i32[2] = ez | 0;
            if (extra.byteLength > 0) {
              bytes.set(extra.subarray(0, Math.min(extra.byteLength, 256 - 12)), 12);
            }
            dev.queue.writeBuffer(ubuf, 0, bytes);
            resources.push({ binding: bind.binding, resource: { buffer: ubuf } });
          } else {
            const id = ids[storageIdx++] | 0;
            const b = buffers.get(id);
            if (!b) throw new Error(`missing buffer id ${id} for binding ${bind.binding}`);
            if (!b.gpuBuffer) {
              b.gpuBuffer = dev.createBuffer({
                size: Math.max(4, b.nbytes),
                usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC,
              });
              if (b.cpu) {
                const bytes = b.cpu instanceof Uint8Array
                  ? b.cpu
                  : new Uint8Array(b.cpu.buffer, b.cpu.byteOffset, b.cpu.byteLength);
                dev.queue.writeBuffer(b.gpuBuffer, 0, bytes);
              }
            }
            resources.push({ binding: bind.binding, resource: { buffer: b.gpuBuffer } });
          }
        }
        const bg = dev.createBindGroup({ layout: pipe.layout, entries: resources });
        const wg = meta.workgroup || [64, 1, 1];
        const gx = Math.max(1, Math.ceil((ex | 0) / (wg[0] || 64)));
        const gy = Math.max(1, Math.ceil((ey | 0) / (wg[1] || 1)));
        const gz = Math.max(1, Math.ceil((ez | 0) / (wg[2] || 1)));
        const encoder = dev.createCommandEncoder();
        const pass = encoder.beginComputePass();
        pass.setPipeline(pipe.pipeline);
        pass.setBindGroup(0, bg);
        pass.dispatchWorkgroups(gx, gy, gz);
        pass.end();
        dev.queue.submit([encoder.finish()]);
        await dev.queue.onSubmittedWorkDone();
        return 0;
      } catch (e) {
        console.error("Dream gpuDispatch:", e);
        return classifyErr(e);
      }
    },

    gpuShaderFromWgsl: (source, entry) => {
      const id = nextId++;
      shaders.set(id, { source: String(source), entry: String(entry) });
      return id;
    },
    gpuDispatchShader: async (shaderId, bufferIds, wx, wy, wz) => {
      const s = shaders.get(shaderId);
      if (!s) return ERR_OTHER;
      const prev = wgslSource;
      const prevAbi = gpuAbi;
      wgslSource = null;
      gpuAbi = {
        kernels: [{
          name: `__raw_${shaderId}`,
          entry: s.entry,
          workgroup: [wx || 64, wy || 1, wz || 1],
          bindings: (bufferIds || []).map((_, i) => ({
            name: `b${i}`, binding: i, kind: "storage", type: "f32", read_write: true,
          })),
        }],
      };
      const inline = s.source;
      const oldLoad = loadWgslText;
      loadWgslText = async () => inline;
      try {
        return await host.gpuDispatch(
          `__raw_${shaderId}`, bufferIds, wx || 1, wy || 1, wz || 1, [],
        );
      } finally {
        loadWgslText = oldLoad;
        wgslSource = prev;
        gpuAbi = prevAbi;
      }
    },

    gpuTextureCreateRgba8: (width, height) => {
      const id = nextId++;
      const w = Math.max(1, width | 0);
      const h = Math.max(1, height | 0);
      textures.set(id, { texture: null, width: w, height: h, cpu: new Uint8Array(w * h * 4) });
      return id;
    },
    gpuTextureWriteRgba: async (id, pixels, x, y, w, h) => {
      try {
        const t = textures.get(id);
        if (!t) throw new Error(`unknown GpuTexture ${id}`);
        const px = Math.max(0, x | 0);
        const py = Math.max(0, y | 0);
        const pw = Math.max(0, w | 0);
        const ph = Math.max(0, h | 0);
        const src = toU8(pixels);
        for (let row = 0; row < ph; row++) {
          const dstOff = ((py + row) * t.width + px) * 4;
          const srcOff = row * pw * 4;
          t.cpu.set(src.subarray(srcOff, srcOff + pw * 4), dstOff);
        }
        const dev = await ensureDevice();
        if (!t.texture) {
          t.texture = dev.createTexture({
            size: [t.width, t.height],
            format: "rgba8unorm",
            usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC,
          });
        }
        dev.queue.writeTexture(
          { texture: t.texture, origin: [px, py] },
          src,
          { bytesPerRow: pw * 4 },
          [pw, ph],
        );
        return 0;
      } catch (e) {
        console.error("Dream gpuTextureWriteRgba:", e);
        return classifyErr(e);
      }
    },
    gpuTextureReadRgba: async (id) => {
      const t = textures.get(id);
      if (!t) throw new Error(`unknown GpuTexture ${id}`);
      return Array.from(t.cpu);
    },

    gpuSurfaceFromCanvas: (canvasId) => {
      if (typeof document === "undefined") return -1;
      const el = document.getElementById(String(canvasId)) || document.querySelector("canvas");
      if (!el || typeof el.getContext !== "function") return -1;
      const id = nextId++;
      surfaces.set(id, {
        canvas: el,
        context: null,
        width: el.width || 1,
        height: el.height || 1,
        configured: false,
        lastTexture: null,
      });
      return id;
    },
    gpuSurfaceConfigure: (id, width, height) => {
      const s = surfaces.get(id);
      if (!s) throw new Error(`unknown GpuSurface ${id}`);
      s.width = Math.max(1, width | 0);
      s.height = Math.max(1, height | 0);
      s.canvas.width = s.width;
      s.canvas.height = s.height;
      s.configured = false;
    },
    gpuSurfacePresent: async (id) => {
      // Present is implicit in blit for v1 (canvas context swap).
      return surfaces.has(id) ? 0 : ERR_OTHER;
    },
    gpuRenderBlit: async (surfaceId, textureId) => {
      try {
        const s = surfaces.get(surfaceId);
        const t = textures.get(textureId);
        if (!s || !t) throw new Error("blit: bad surface/texture id");
        const dev = await ensureDevice();
        await ensureBlit(dev);
        if (!t.texture) {
          t.texture = dev.createTexture({
            size: [t.width, t.height],
            format: "rgba8unorm",
            usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC,
          });
          if (t.cpu) {
            dev.queue.writeTexture(
              { texture: t.texture },
              t.cpu,
              { bytesPerRow: t.width * 4 },
              [t.width, t.height],
            );
          }
        }
        if (!s.context) {
          s.context = s.canvas.getContext("webgpu");
          if (!s.context) throw new Error("canvas webgpu context unavailable");
        }
        if (!s.configured) {
          s.context.configure({
            device: dev,
            format: navigator.gpu.getPreferredCanvasFormat(),
            alphaMode: "opaque",
          });
          s.configured = true;
        }
        const view = s.context.getCurrentTexture().createView();
        const bg = dev.createBindGroup({
          layout: blitBindLayout,
          entries: [
            { binding: 0, resource: blitSampler },
            { binding: 1, resource: t.texture.createView() },
          ],
        });
        const encoder = dev.createCommandEncoder();
        const pass = encoder.beginRenderPass({
          colorAttachments: [{
            view,
            clearValue: { r: 0, g: 0, b: 0, a: 1 },
            loadOp: "clear",
            storeOp: "store",
          }],
        });
        pass.setPipeline(blitPipeline);
        pass.setBindGroup(0, bg);
        pass.draw(3);
        pass.end();
        dev.queue.submit([encoder.finish()]);
        await dev.queue.onSubmittedWorkDone();
        return 0;
      } catch (e) {
        console.error("Dream gpuRenderBlit:", e);
        return classifyErr(e);
      }
    },
  };

  return host;
}
