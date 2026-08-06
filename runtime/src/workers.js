import { isNode } from "./platform.js";

/**
 * Source of the worker bootstrap module. It imports this same `dream.js` (so the worker reuses
 * all the env/`Dream` import wiring, including nested workers) and, on `init`, instantiates the
 * same `.wasm` bytes importing the parent's shared `WebAssembly.Memory`. Thereafter each `msg` or
 * `dispatch` runs a `fun(string):string` body via `__workerInvoke` and posts the reply back.
 *
 * Browser workers use `self.onmessage` / `self.postMessage`; Node `worker_threads` workers use
 * `parentPort` instead — pass `node: true` for that dialect.
 */
function workerBootSource(dreamUrl, { node = false } = {}) {
  if (node) {
    return `import { parentPort } from 'node:worker_threads';
import * as Dream from ${JSON.stringify(dreamUrl)};
let inst = null;
parentPort.on('message', async (m) => {
  if (m.t === 'init') {
    inst = await Dream.load(m.bytes, { abi: m.abi, memory: m.memory });
    parentPort.postMessage({ t: 'ready' });
  } else if (m.t === 'msg') {
    parentPort.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'dispatch') {
    parentPort.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'term') {
    parentPort.close();
  }
});
`;
  }
  return `import * as Dream from ${JSON.stringify(dreamUrl)};
let inst = null;
self.onmessage = async (e) => {
  const m = e.data;
  if (m.t === 'init') {
    inst = await Dream.load(m.bytes, { abi: m.abi, memory: m.memory });
    self.postMessage({ t: 'ready' });
  } else if (m.t === 'msg') {
    self.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'dispatch') {
    self.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'term') {
    self.close();
  }
};
`;
}

/**
 * Builds the `Dream`-module worker host functions (`workerSpawn`/`workerPost`/`workerRecv`/
 * `workerTerminate`/`workerPoolSpawn`/`workerPoolDispatch`) behind `src/stdlib/core/webworker.dream`.
 * Each worker is a real browser `Worker` or Node `worker_threads.Worker` running a fresh instance
 * of the same module, importing the parent's shared `WebAssembly.Memory`.
 * `workerRecv`/`workerPoolDispatch` are `extern async`, so they return Promises bridged into
 * Dream's scheduler.
 */
function makeWorkerModule(wasmBytes, abi, getSharedMemory) {
  const reg = new Map();
  let nextId = 1;
  /** Lazily resolved Node `worker_threads.Worker` constructor (null until first Node spawn). */
  let NodeWorkerCtor = null;

  const postJob = (state, job) => {
    if (state.ready) state.worker.postMessage(job);
    else state.queued.push(job);
  };

  const attachHandlers = (worker, state) => {
    // Browser: `onmessage` event with `e.data`. Node worker_threads: `message` event with data directly.
    if (typeof worker.on === "function" && isNode) {
      worker.on("message", (m) => {
        if (m.t === "ready") {
          state.ready = true;
          for (const q of state.queued) state.worker.postMessage(q);
          state.queued = [];
        } else if (m.t === "reply") {
          if (state.pending.length > 0) state.pending.shift()(m.data);
          else state.replies.push(m.data);
        }
      });
    } else {
      worker.onmessage = (e) => {
        const m = e.data;
        if (m.t === "ready") {
          state.ready = true;
          for (const q of state.queued) state.worker.postMessage(q);
          state.queued = [];
        } else if (m.t === "reply") {
          if (state.pending.length > 0) state.pending.shift()(m.data);
          else state.replies.push(m.data);
        }
      };
    }
  };

  const spawnWorker = (fnIndex, env) => {
    const state = {
      worker: null,
      fnIndex,
      env,
      pending: [],
      replies: [],
      ready: false,
      queued: [],
      blobUrl: null,
    };

    const finishSpawn = (worker) => {
      state.worker = worker;
      attachHandlers(worker, state);
      worker.postMessage({
        t: "init",
        bytes: wasmBytes,
        abi,
        memory: getSharedMemory(),
      });
    };

    if (isNode) {
      // Spawn synchronously once the ctor is cached; first call kicks off an async import and
      // queues the init until it resolves (postJob already buffers until `ready`).
      const startNode = (NodeWorker) => {
        const worker = new NodeWorker(workerBootSource(import.meta.url, { node: true }), {
          eval: true,
        });
        // Allow the Node process to exit once Dream's async main has settled even if a worker
        // handle is still referenced briefly during teardown.
        if (typeof worker.unref === "function") worker.unref();
        finishSpawn(worker);
      };
      if (NodeWorkerCtor) {
        startNode(NodeWorkerCtor);
      } else {
        // Synchronous-looking spawn from Dream's POV: register the id immediately; jobs queue
        // until the worker posts `ready`. The dynamic import of worker_threads is one-shot.
        import("node:worker_threads").then(({ Worker }) => {
          NodeWorkerCtor = Worker;
          startNode(Worker);
        });
      }
    } else if (typeof Worker !== "undefined") {
      const url = URL.createObjectURL(
        new Blob([workerBootSource(import.meta.url)], { type: "text/javascript" }),
      );
      state.blobUrl = url;
      finishSpawn(new Worker(url, { type: "module" }));
    } else {
      throw new Error(
        "WebWorker requires a browser Worker or Node worker_threads; neither is available in this environment",
      );
    }

    const id = nextId++;
    reg.set(id, state);
    return id;
  };

  return {
    // `body` arrives marshaled as a JS callable wrapping the Dream funcref; recover its raw funcidx
    // and closure-env word (0 for a non-capturing body).
    workerSpawn: (body, env) =>
      spawnWorker(body && body.__dreamFuncIndex != null ? body.__dreamFuncIndex : body, env),
    workerPoolSpawn: () => spawnWorker(0, 0),
    workerPost: (id, msg) => {
      const s = reg.get(id);
      if (!s) return;
      postJob(s, { t: "msg", fnIdx: s.fnIndex, env: s.env, data: msg });
    },
    workerPoolDispatch: (id, fnIndex, env, msg) =>
      new Promise((resolve) => {
        const s = reg.get(id);
        if (!s) return resolve("");
        s.pending.push(resolve);
        postJob(s, { t: "dispatch", fnIdx: fnIndex, env, data: msg });
      }),
    // extern async: resolve with the next reply (or "" if the worker is gone).
    workerRecv: (id) =>
      new Promise((resolve) => {
        const s = reg.get(id);
        if (!s) return resolve("");
        if (s.replies.length > 0) resolve(s.replies.shift());
        else s.pending.push(resolve);
      }),
    workerTerminate: (id) => {
      const s = reg.get(id);
      if (!s) return;
      try {
        if (s.worker) {
          s.worker.postMessage({ t: "term" });
          s.worker.terminate();
        }
      } catch (_) {
        /* already gone */
      }
      if (s.blobUrl) {
        try {
          URL.revokeObjectURL(s.blobUrl);
        } catch (_) {
          /* ignore */
        }
      }
      for (const p of s.pending) p("");
      reg.delete(id);
    },
  };
}
