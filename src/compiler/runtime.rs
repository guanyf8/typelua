//! 共享运行时的源码。传目录编译时，它被原样写到输出根目录的
//! `__typelua_runtime.lua`；每个生成的模块文件头都会 `require("__typelua_runtime")`。
//! 只列文件时产物不依赖这个文件：同一份源码被内联进文件头，并登记到
//! `package.loaded["__typelua_runtime"]`，两边的注册表因此是同一份。
//!
//! 放在独立的 .lua 里而不是 Rust 字符串字面量：它是给人读、也给人改的 Lua 代码，
//! 混进 Rust 源码只会让两边都难读
pub const SOURCE: &str = include_str!("runtime.lua");
