pub mod emitter;
pub mod linter;
/// 多模块工程的共享运行时（`__typelua_runtime.lua` 的源码）
pub mod runtime;

/// 共享运行时在每个模块里占的局域名。用户占用它时 linter 直接报错 ——
/// 注入的 `local __typelua = require(…)` 会和用户的值互相覆盖
pub const RUNTIME_LOCAL: &str = "__typelua";

/// 运行时模块的 require 名，也是 `--out` 目录里那个文件名
pub const RUNTIME_MODULE: &str = "__typelua_runtime";
mod utils;
