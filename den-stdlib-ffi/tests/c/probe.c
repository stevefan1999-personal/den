/* The scalar vocabulary den:ffi claims to marshal, one function per shape,
 * plus the two exported variables the static-symbol form reads. Built at test
 * time by tests/ffi.rs with `cc -shared -fPIC`, so the .so is always this
 * compiler's ABI. */

#include <stddef.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <pthread.h>

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

/* Callbacks. `apply` is the same-thread case: den's trampoline runs inside the
 * very call JS made, so the realm's runtime lock is held throughout. */
int32_t apply(int32_t (*function)(int32_t), int32_t value) { return function(value); }

double apply_f64(double (*function)(double), double value) { return function(value); }

void notify(void (*function)(int32_t), int32_t value) { function(value); }

/* qsort-shaped: C owns the loop and calls back once per comparison, handing
 * the callback two addresses rather than values.
 *
 * The values live here rather than in a `buffer` argument because a callback
 * forces the symbol to be `nonblocking`, and a nonblocking call may not borrow
 * a script's bytes. JS loads them, sorts them and reads them back by address. */
static int32_t sorted[8];
static size_t sorted_count = 0;

void load_values(const uint8_t *bytes, size_t length) {
  if (length > sizeof(sorted)) {
    length = sizeof(sorted);
  }
  memcpy(sorted, bytes, length);
  sorted_count = length / sizeof(int32_t);
}

const int32_t *sorted_values(void) { return sorted; }

void sort_values(int (*compare)(const void *, const void *)) {
  qsort(sorted, sorted_count, sizeof(int32_t), compare);
}

/* The foreign-thread seam: C fires the callback from a thread it created and
 * joins. den has no way into the realm from there, so phase 4 answers the zero
 * value and says so on stderr; phase 5 posts to a mailbox instead. */
struct fired {
  int32_t (*function)(int32_t);
  int32_t value;
  int32_t result;
};

static void *fire(void *raw) {
  struct fired *call = raw;
  call->result = call->function(call->value);
  return NULL;
}

int32_t call_on_thread(int32_t (*function)(int32_t), int32_t value) {
  struct fired call = {function, value, -1};
  pthread_t thread;
  if (pthread_create(&thread, NULL, fire, &call) != 0) {
    return -1;
  }
  pthread_join(thread, NULL);
  return call.result;
}

/* A callback C keeps for itself, which is how a callback outlives the call
 * that handed it over. `fire_stored` runs it on the caller's own thread — the
 * realm's, when a synchronous symbol calls it — and `fire_on_thread_and_join`
 * runs it on a thread it creates and then waits for, which is §4.7's residual
 * deadlock: the realm is inside this function and so cannot answer. */
static int32_t (*stored)(int32_t) = NULL;

void store_callback(int32_t (*function)(int32_t)) { stored = function; }

int32_t fire_stored(int32_t value) { return stored ? stored(value) : -1; }

int32_t fire_on_thread_and_join(int32_t value) {
  struct fired call = {stored, value, -1};
  pthread_t thread;
  if (stored == NULL || pthread_create(&thread, NULL, fire, &call) != 0) {
    return -1;
  }
  pthread_join(thread, NULL);
  return call.result;
}

/* Structs by value, one per ABI class this target has.
 *
 * `Point` is 8 bytes and goes in a single INTEGER register; `Triple` is 24 and
 * is MEMORY class, which is what makes libffi allocate the hidden `sret`
 * pointer. A marshaller that only handles the first passes `point_scale` and
 * corrupts the stack on `triple_sum`. */
typedef struct {
  int32_t x;
  int32_t y;
} Point;

Point point_scale(Point point, int32_t factor) {
  Point scaled = {point.x * factor, point.y * factor};
  return scaled;
}

typedef struct {
  double a;
  double b;
  double c;
} Triple;

Triple triple_sum(Triple triple, double addend) {
  Triple summed = {triple.a + addend, triple.b + addend, triple.c + addend};
  return summed;
}

/* The padding case: this compiler decides where `n` sits, and den has to have
 * computed the same offset or the sum comes back wrong. */
typedef struct {
  int8_t b;
  int32_t n;
} Padded;

int32_t padded_sum(Padded padded) { return padded.b + padded.n; }

/* A struct inside a struct, which is where a layout that only handles scalars
 * stops being right. */
typedef struct {
  Point origin;
  int32_t z;
} Nested;

int32_t nested_sum(Nested nested) {
  return nested.origin.x + nested.origin.y + nested.z;
}

/* An exported struct variable, for the `{ type: { struct } }` form. */
Point base_point = {2, 3};
