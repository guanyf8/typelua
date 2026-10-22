//! 让 cdylib 把 Lua C API 留成未定义符号。
//!
//! Lua 模块不链接 liblua：宿主进程里已经有这些符号，加载时由 dyld 解析。
//! macOS 的链接器默认不允许共享库留未定义符号，得显式放行；Linux 本来就可以。
//! 只对 cdylib 生效 —— bin 和测试照常静态链接，不受影响。

fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
