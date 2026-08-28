const seen = [];
__natives.reportException = (value) => seen.push(value);
const failure = new TypeError("boom");
reportError(failure);
let arity = "no throw";
try { reportError(); } catch (error) { arity = error.constructor.name }
`${seen.length},${seen[0] === failure},${arity},${reportError.length}`
