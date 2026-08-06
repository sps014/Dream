/** Default `env` builtins every Dream module imports (mirrors src/.../wasm_runner.rs). */
function defaultEnv(getInstance, options) {
  const writeOut = options.stdout || ((s) => (typeof process !== "undefined" ? process.stdout.write(s) : console.log(s)));
  const writeLine = options.stdout
    ? (s) => options.stdout(s + "\n")
    : (s) => console.log(s);

  return {
    print_string: (ptr) => writeOut(getInstance().readString(ptr)),
    println: (ptr) => writeLine(getInstance().readString(ptr)),
    print_int: (v) => writeOut(String(v)),
    print_float: (v) => writeOut(String(v)),
    print_double: (v) => writeOut(String(v)),
    print_char: (v) => writeOut(String.fromCharCode(v)),
    sin: Math.sin,
    cos: Math.cos,
    tan: Math.tan,
    asin: Math.asin,
    acos: Math.acos,
    atan: Math.atan,
    atan2: Math.atan2,
    abs: Math.abs,
    sqrt: Math.sqrt,
    pow: Math.pow,
    floor: Math.floor,
    ceil: Math.ceil,
    round: Math.round,
  };
}
