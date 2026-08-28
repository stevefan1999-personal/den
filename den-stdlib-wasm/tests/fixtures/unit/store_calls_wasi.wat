(module
  (import "wasi_snapshot_preview1" "environ_sizes_get" (func $sizes (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "run") (result i32) (call $sizes (i32.const 0) (i32.const 8))))
