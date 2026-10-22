//! TypeLua 的编译前端，也是 Lua loader 的宿主入口。
//!
//! bin 和 cdylib 共用这棵树：`main.rs` 走完整的 check + emit，`ffi.rs` 只走
//! parse + emit。把模块放在 lib 里，测试也只编一遍。

pub mod api;
pub mod compiler;
mod ffi;
pub mod lexer;
pub mod parser;
