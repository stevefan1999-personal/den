/* The scalar vocabulary den:ffi claims to marshal, one function per shape,
 * plus the two exported variables the static-symbol form reads. Built at test
 * time by tests/ffi.rs with `cc -shared -fPIC`, so the .so is always this
 * compiler's ABI. */

#include <stddef.h>
#include <stdint.h>

int add(int left, int right) { return left + right; }

double mul_f64(double left, double right) { return left * right; }

void no_op(void) {}

/* Every width echoes its argument: a round trip is what catches a marshaller
 * that widens, wraps or truncates. */
int8_t echo_i8(int8_t value) { return value; }
uint8_t echo_u8(uint8_t value) { return value; }
int16_t echo_i16(int16_t value) { return value; }
uint16_t echo_u16(uint16_t value) { return value; }
uint32_t echo_u32(uint32_t value) { return value; }
int64_t echo_i64(int64_t value) { return value; }
uint64_t echo_u64(uint64_t value) { return value; }
intptr_t echo_isize(intptr_t value) { return value; }
size_t echo_usize(size_t value) { return value; }
float echo_f32(float value) { return value; }

/* A sub-register return with no argument to carry it: the case where a cell
 * sized to a register rather than to the type would show. */
int8_t minus_one_i8(void) { return -1; }

_Bool negate(_Bool value) { return !value; }

/* Pointers: one out, one in, one null, and two C strings. */
static int32_t the_cell = 7;
int32_t *cell_address(void) { return &the_cell; }
int32_t read_i32(const int32_t *address) { return *address; }
void *null_pointer(void) { return NULL; }

const char *hello(void) { return "hello"; }

/* Eight bytes with no terminator, for the bounded-scan refusal. */
static const char unterminated_bytes[8] = {'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'};
const char *unterminated(void) { return unterminated_bytes; }

/* Exported variables, read by the `{ type }` schema form. */
int32_t version = 3;
double ratio = 0.5;

/* The buffer argument: C writes through the pointer den hands it, so a view
 * built at a byteOffset shows exactly where that pointer landed. Each byte is
 * its own index plus one, which no zeroed buffer can be mistaken for. */
void fill_bytes(uint8_t *out, size_t length) {
  for (size_t index = 0; index < length; index++) {
    out[index] = (uint8_t)(index + 1);
  }
}
