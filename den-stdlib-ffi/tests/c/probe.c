/* The phase-1 vocabulary, and nothing else: one integer function, one
 * double function, one void function. Built at test time by tests/ffi.rs
 * with `cc -shared -fPIC`, so the .so is always this compiler's ABI. */

int add(int left, int right) { return left + right; }

double mul_f64(double left, double right) { return left * right; }

void no_op(void) {}
