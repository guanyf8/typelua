//! Lua C 模块：让普通 Lua 直接 `require` 一份 `.tua`。
//!
//! 需要 Lua 5.2+（`package.searchers` 与 `luaL_loadbufferx`）。
//!
//! 构建：
//!
//! ```text
//! cargo build --release
//! cp target/release/libtypelua.dylib typelua.so     # Linux 上是 libtypelua.so
//! ```
//!
//! 使用：
//!
//! ```lua
//! package.cpath = "./?.so;" .. package.cpath
//! require("typelua")                 -- 安装搜索器
//! local account = require("pkg.account")
//! ```
//!
//! `require("typelua")` 做三件事：把 `__typelua_runtime` 放进
//! `package.preload`，往 `package.searchers` 末尾追加一个 `.tua` 搜索器，
//! 返回带 `compile` 的模块表。搜索器只 parse + emit，不跑 checker ——
//! checker 的保留名规则保证按名字重写在合法程序上和 checker 的结论一致。
//!
//! 模块身份就是 require 的字符串本身：`require("pkg.account")` 的模块名是
//! `pkg.account`，别的文件里 `import {..} in "pkg.account"` 才对得上。
//! `require("pkg/account.tua")` 也能加载，但那个模块名就是
//! `pkg/account.tua`；同一个文件别用两种写法 require，否则会执行两遍。

use std::ffi::{CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use crate::api::compile_source;
use crate::compiler::runtime;

type LuaState = c_void;
type LuaCFunction = unsafe extern "C" fn(*mut LuaState) -> c_int;

const LUA_TSTRING: c_int = 4;
const LUA_TTABLE: c_int = 5;

// Lua 模块不链接 liblua：这些符号由宿主进程在加载时提供（见 build.rs）。
unsafe extern "C" {
    fn lua_settop(l: *mut LuaState, idx: c_int);
    fn lua_type(l: *mut LuaState, idx: c_int) -> c_int;
    fn lua_tolstring(l: *mut LuaState, idx: c_int, len: *mut usize) -> *const c_char;
    fn lua_pushlstring(l: *mut LuaState, s: *const c_char, len: usize) -> *const c_char;
    fn lua_pushnil(l: *mut LuaState);
    fn lua_pushcclosure(l: *mut LuaState, f: LuaCFunction, n: c_int);
    fn lua_createtable(l: *mut LuaState, narr: c_int, nrec: c_int);
    fn lua_getglobal(l: *mut LuaState, name: *const c_char);
    fn lua_getfield(l: *mut LuaState, idx: c_int, key: *const c_char);
    fn lua_setfield(l: *mut LuaState, idx: c_int, key: *const c_char);
    fn lua_rawseti(l: *mut LuaState, idx: c_int, n: i64);
    fn lua_rawlen(l: *mut LuaState, idx: c_int) -> usize;
    fn luaL_loadbufferx(
        l: *mut LuaState,
        buf: *const c_char,
        len: usize,
        name: *const c_char,
        mode: *const c_char,
    ) -> c_int;
    fn lua_error(l: *mut LuaState) -> c_int;
}

/// `require("typelua")` 的入口。安装运行时和搜索器，返回模块表。
///
/// # Safety
/// 只能由 Lua 解释器在 `lua_State` 有效时调用。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_typelua(l: *mut LuaState) -> c_int {
    let setup = catch_unwind(AssertUnwindSafe(|| unsafe {
        install_runtime(l)?;
        install_searcher(l)?;
        Ok::<(), String>(())
    }));

    match setup {
        Ok(Ok(())) => {}
        Ok(Err(message)) => return unsafe { raise(l, message) },
        Err(_) => return unsafe { raise(l, "typelua: 初始化时内部 panic".to_string()) },
    }

    unsafe {
        lua_createtable(l, 0, 2);
        lua_pushcclosure(l, typelua_compile, 0);
        lua_setfield(l, -2, c"compile".as_ptr());
        push_string(l, env!("CARGO_PKG_VERSION"));
        lua_setfield(l, -2, c"version".as_ptr());
    }
    1
}

/// `typelua.compile(source [, module]) -> chunk | nil, err`。
///
/// 给测试和「自己实现搜索器」的宿主用；正常 `require` 走下面的 searcher。
/// 返回的文本还没有经过 `load`，也不会写盘。
unsafe extern "C" fn typelua_compile(l: *mut LuaState) -> c_int {
    let source = match unsafe { read_lua_string(l, 1) } {
        Some(source) => source,
        None => {
            unsafe {
                lua_pushnil(l);
                push_string(l, "typelua.compile: 第一个参数必须是 UTF-8 字符串");
            }
            return 2;
        }
    };
    let module = match unsafe { read_lua_string(l, 2) } {
        Some(module) => module,
        None => "inline".to_string(),
    };
    if let Err(message) = validate_module_name(&module) {
        unsafe {
            lua_pushnil(l);
            push_string(l, &message);
        }
        return 2;
    }

    let outcome = catch_unwind(AssertUnwindSafe(|| compile_source(&module, &source)));
    match outcome {
        Ok(Ok(chunk)) => {
            unsafe { push_string(l, &chunk) };
            1
        }
        Ok(Err(errors)) => {
            unsafe {
                lua_pushnil(l);
                push_string(l, &errors.join("\n"));
            }
            2
        }
        Err(_) => {
            unsafe {
                lua_pushnil(l);
                push_string(l, "typelua.compile: 内部 panic");
            }
            2
        }
    }
}

/// `package.searchers` 里的一员：模块名 -> 编译好的 chunk。
///
/// 找不到时按 Lua 的约定返回一条字符串，让其它搜索器继续试；找到了但
/// 读不了/解析失败则直接抛错 —— 文件存在却坏了，不该退化成「模块不存在」。
unsafe extern "C" fn typelua_searcher(l: *mut LuaState) -> c_int {
    let name = match unsafe { read_lua_string(l, 1) } {
        Some(name) => name,
        None => {
            unsafe { push_string(l, "\n\t.tua 模块名必须是 UTF-8 字符串") };
            return 1;
        }
    };

    // package.path 必须在 Rust 工作开始前读走：下面的 catch_unwind 里不再碰 Lua
    let package_path = unsafe { package_path(l) }.unwrap_or_default();
    let outcome = catch_unwind(AssertUnwindSafe(|| load_module(&name, &package_path)));
    drop(package_path);

    match outcome {
        Ok(Ok(chunk)) => unsafe {
            let chunkname = CString::new(format!("=typelua({name})")).unwrap_or_default();
            let status = luaL_loadbufferx(
                l,
                chunk.as_ptr().cast(),
                chunk.len(),
                chunkname.as_ptr(),
                c"t".as_ptr(),
            );
            drop(chunkname);
            drop(chunk);
            if status == 0 {
                1
            } else {
                lua_error(l) // 栈顶是 luaL_loadbufferx 留的错误信息
            }
        },
        Ok(Err(LoadError::NotFound(message))) => {
            unsafe { push_string(l, &message) };
            1
        }
        Ok(Err(LoadError::Failed(message))) => unsafe { raise(l, message) },
        Err(_) => unsafe { raise(l, format!("typelua: 编译 {name} 时内部 panic")) },
    }
}

enum LoadError {
    /// 没有这个文件：交给后续搜索器，最终由 require 汇总
    NotFound(String),
    /// 文件在但读不了/解析失败：直接抛
    Failed(String),
}

/// 找文件 -> 读源 -> parse+emit。纯 Rust，不碰 Lua 栈。
fn load_module(name: &str, package_path: &str) -> Result<String, LoadError> {
    validate_module_name(name).map_err(LoadError::Failed)?;

    let candidates = candidate_paths(name, package_path);
    let Some(path) = candidates.iter().find(|path| Path::new(path).is_file()) else {
        return Err(LoadError::NotFound(not_found_message(name, &candidates)));
    };

    let source = std::fs::read_to_string(path)
        .map_err(|err| LoadError::Failed(format!("{path}: 读取失败: {err}")))?;
    compile_source(name, &source).map_err(|errors| {
        let message = errors
            .iter()
            .map(|error| format!("{path}:{error}"))
            .collect::<Vec<_>>()
            .join("\n");
        LoadError::Failed(message)
    })
}

/// 保护发射端的字符串插值：模块名会原样进 `enter("...")`/`get("...", "..")`。
fn validate_module_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("typelua: 模块名为空".to_string());
    }
    if let Some(ch) = name
        .chars()
        .find(|&ch| ch == '"' || ch == '\\' || ch.is_control())
    {
        return Err(format!("typelua: 模块名 {name:?} 含有不支持的字符 {ch:?}"));
    }
    Ok(())
}

/// 把 `package.path` 的模板改成 `.tua`。只替换结尾的 `.lua`，目录名里带
/// `.lua` 的（例如 `.../lua.lua/?.lua`）不会被误伤。
fn tua_templates(package_path: &str) -> Vec<String> {
    let mut templates = Vec::new();
    for entry in package_path.split(';') {
        let entry = entry.trim();
        if entry.is_empty() || !entry.contains('?') {
            continue;
        }
        let template = match entry.strip_suffix(".lua") {
            Some(stem) => format!("{stem}.tua"),
            None => entry.to_string(),
        };
        if !templates.contains(&template) {
            templates.push(template);
        }
    }
    // package.path 被宿主改空时至少试当前目录
    if templates.is_empty() {
        templates.push("./?.tua".to_string());
    }
    templates
}

/// 模块名 -> 候选文件路径。点号按 Lua 的惯例映射成目录分隔符；
/// 以 `.tua` 结尾的名字按字面量处理，同时兼容 `pkg/account.tua` 和
/// `pkg.account.tua` 两种写法。
fn candidate_paths(name: &str, package_path: &str) -> Vec<String> {
    let templates = tua_templates(package_path);
    let literal = name.ends_with(".tua");
    let stem = if literal {
        &name[..name.len() - ".tua".len()]
    } else {
        name
    };

    let mut stems = Vec::new();
    if literal {
        stems.push(stem.to_string());
        if stem.contains('.') {
            stems.push(stem.replace('.', "/"));
        }
    } else {
        stems.push(name.replace('.', "/"));
    }

    let mut candidates = Vec::new();
    for template in &templates {
        for stem in &stems {
            let candidate = template.replace('?', stem);
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    // 绝对路径 / 当前目录下的裸文件名
    if literal && !candidates.contains(&name.to_string()) {
        candidates.push(name.to_string());
    }
    candidates
}

fn not_found_message(name: &str, candidates: &[String]) -> String {
    let shown = candidates
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let more = if candidates.len() > 3 { ", ..." } else { "" };
    format!("\n\tno .tua file for '{name}' (tried {shown}{more})")
}

/// 把 `__typelua_runtime` 注册进 `package.preload`：加载第一个 `.tua` 时
/// 不需要磁盘上再有一份 runtime.lua。
unsafe fn install_runtime(l: *mut LuaState) -> Result<(), String> {
    unsafe {
        lua_settop(l, 0);
        lua_getglobal(l, c"package".as_ptr());
        if lua_type(l, -1) != LUA_TTABLE {
            lua_settop(l, 0);
            return Err("typelua: package 表不可用".to_string());
        }
        lua_getfield(l, -1, c"preload".as_ptr());
        if lua_type(l, -1) != LUA_TTABLE {
            lua_settop(l, 0);
            return Err("typelua: package.preload 不是表".to_string());
        }

        let source = runtime::SOURCE;
        let status = luaL_loadbufferx(
            l,
            source.as_ptr().cast(),
            source.len(),
            c"=__typelua_runtime".as_ptr(),
            c"t".as_ptr(),
        );
        if status != 0 {
            let reason = read_lua_string(l, -1).unwrap_or_else(|| "未知错误".to_string());
            lua_settop(l, 0);
            return Err(format!("typelua: __typelua_runtime 预编译失败: {reason}"));
        }

        lua_setfield(l, -2, c"__typelua_runtime".as_ptr());
        lua_settop(l, 0);
        Ok(())
    }
}

/// 追加到 `package.searchers` 末尾：标准 `.lua` 搜索器先跑，同名 `.lua`
/// 存在时不会被 `.tua` 抢走。
unsafe fn install_searcher(l: *mut LuaState) -> Result<(), String> {
    unsafe {
        lua_settop(l, 0);
        lua_getglobal(l, c"package".as_ptr());
        if lua_type(l, -1) != LUA_TTABLE {
            lua_settop(l, 0);
            return Err("typelua: package 表不可用".to_string());
        }
        lua_getfield(l, -1, c"searchers".as_ptr());
        if lua_type(l, -1) != LUA_TTABLE {
            lua_settop(l, 0);
            return Err("typelua: package.searchers 不是表（需要 Lua 5.2+）".to_string());
        }

        let count = lua_rawlen(l, -1);
        lua_pushcclosure(l, typelua_searcher, 0);
        lua_rawseti(l, -2, (count + 1) as i64);
        lua_settop(l, 0);
        Ok(())
    }
}

/// 读 `package.path`。读完清栈 —— 调用方（searcher）已经把模块名拷进 Rust 了。
unsafe fn package_path(l: *mut LuaState) -> Option<String> {
    unsafe {
        lua_settop(l, 0);
        lua_getglobal(l, c"package".as_ptr());
        if lua_type(l, -1) != LUA_TTABLE {
            lua_settop(l, 0);
            return None;
        }
        lua_getfield(l, -1, c"path".as_ptr());
        let path = read_lua_string(l, -1);
        lua_settop(l, 0);
        path
    }
}

/// 读一个 Lua 字符串参数。非字符串或非 UTF-8 一律 `None`。
unsafe fn read_lua_string(l: *mut LuaState, idx: c_int) -> Option<String> {
    unsafe {
        if lua_type(l, idx) != LUA_TSTRING {
            return None;
        }
        let mut len = 0usize;
        let ptr = lua_tolstring(l, idx, &mut len);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), len);
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    }
}

unsafe fn push_string(l: *mut LuaState, value: &str) {
    unsafe { lua_pushlstring(l, value.as_ptr().cast(), value.len()) };
}

/// 用 Rust 的错误文本抛 Lua 错误。先清栈、再 push、把 String 放掉，最后
/// `lua_error` —— longjmp 发生时栈上没有还活着的 Rust 析构对象。
unsafe fn raise(l: *mut LuaState, message: String) -> c_int {
    unsafe {
        lua_settop(l, 0);
        lua_pushlstring(l, message.as_ptr().cast(), message.len());
    }
    drop(message);
    unsafe { lua_error(l) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_names_map_to_directories() {
        let paths = candidate_paths("pkg.account", "./?.lua;/opt/lua/5.3/?.lua");
        assert_eq!(paths, ["./pkg/account.tua", "/opt/lua/5.3/pkg/account.tua"]);
    }

    #[test]
    fn literal_tua_names_are_searched_as_written() {
        let paths = candidate_paths("a.tua", "./?.lua");
        assert!(paths.contains(&"./a.tua".to_string()));
        assert!(paths.contains(&"a.tua".to_string()));
    }

    #[test]
    fn literal_slashed_names_do_not_get_dotted() {
        let paths = candidate_paths("pkg/account.tua", "./?.lua");
        assert!(paths.contains(&"./pkg/account.tua".to_string()));
    }

    #[test]
    fn dotted_tua_names_try_both_spellings() {
        let paths = candidate_paths("pkg.account.tua", "./?.lua");
        assert!(paths.contains(&"./pkg/account.tua".to_string()));
        assert!(paths.contains(&"./pkg.account.tua".to_string()));
    }

    #[test]
    fn package_path_without_lua_entries_falls_back_to_cwd() {
        assert_eq!(tua_templates(""), ["./?.tua"]);
        assert_eq!(tua_templates("/x/?.so"), ["/x/?.so"]);
    }

    #[test]
    fn module_names_may_not_break_the_emitted_literal() {
        assert!(validate_module_name("pkg.account").is_ok());
        assert!(validate_module_name("a.tua").is_ok());
        assert!(validate_module_name("a\"b").is_err());
        assert!(validate_module_name("a\\b").is_err());
        assert!(validate_module_name("a\nb").is_err());
        assert!(validate_module_name("").is_err());
    }
}
