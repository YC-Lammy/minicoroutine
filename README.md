# minicoroutine
a mini coroutine library, a wrapper on minicoro

# Features
- Stackful asymmetric coroutines.
- Supports nesting coroutines (resuming a coroutine from another coroutine).
- Supports no_std and no_alloc.
- Supports custom allocators.

# Supported targets
This crate currently supports the following targets:

|         | Linux | Windows | Mac | IOS | Android | Emscripten |
| ------- |-------|---------|-----|-----| ------- | ---------- |
| x86_64  | ✅   | ✅      | ✅ | ❌ |  ❌     |  ❌        |
| i686    | ✅   | ❌      | ❌ | ❌ |  ❌     |  ❌        |
| AArch64 | ✅   | ❌      | ✅ | ✅ |  ✅     |  ❌        |
| ARM     | ❌   | ❌      | ✅ | ✅ |  ✅     |  ❌        |
| RISC-V  | ✅   | ❌      | ❌ | ❌ |  ❌     |  ❌        |
| Wasm    | ❌   | ❌      | ❌ | ❌ |  ❌     |  ✅        |

# Panic
Panics are not supported by this crate, catch unwind will not be able to catch panics invoked inside the coroutine.
use the yield api to pass on any errors instead.