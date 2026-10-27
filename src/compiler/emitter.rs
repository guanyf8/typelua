mod lua53;
pub use lua53::Lua53;

use super::utils::DFS;
use crate::parser::ast::*;
use crate::parser::parser::*;

/// 运行时怎么进产物。模块注册表（`enter` / `def` / `get`）两种模式都开，
/// 差别只在运行时本体是随文件内联，还是从共享的 `.lua` 里 require
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    /// 只列文件（一个或多个）：运行时贴在文件头，并登记到
    /// `package.loaded["__typelua_runtime"]`，多个这样编译出的文件共用同一张注册表
    Inline,
    /// 传目录 / loader：文件头 require 共享的 `__typelua_runtime.lua`
    Require,
}

/// 一次发射需要的全部工程级信息。和 `ast` 一样当参数传，不挂在 emitter 上：
/// 同一个 emitter 实例要给多个模块复用，存成字段就得每模块改一次可变状态
pub struct EmitCtx<'a> {
    /// 当前模块名（`import {..} in "pkg.account"` 里那个），也是注册表的键
    pub module: &'a str,
    /// 运行时的落地方式。两种模式的类体、import 改写、pub 注册完全一致
    pub runtime: RuntimeMode,
}

pub trait Emitter: DFS<()> {
    /// 整棵树 -> 一份 Lua 文本。发射端只读同一次解析的 CST：import 了谁、
    /// 哪个类声明在后面，这两件事都写在树上，不需要 linter 的产出
    fn render(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>) -> String;

    /// 落盘。原因由实现自己报过，CLI 据此决定退出码
    fn write(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>, target_path: &str) -> bool {
        let buffer = self.render(ast, ctx);
        match std::fs::write(target_path, buffer) {
            Ok(()) => true,
            Err(err) => {
                eprintln!("{}: 写出失败 {target_path}: {err}", ctx.module);
                false
            }
        }
    }
}

/// 解析 + 发射：把一份 `.tua` 源码直接变成 Lua 文本，不经过 linter。
///
/// 这是给宿主（Lua loader / 编辑器插件）用的窄接口：拿到源码字符串就出 Lua，
/// 不需要工程、不需要检查结果。语法错误原样交回，由调用方决定怎么报。
pub fn emit_source(
    module: &str,
    source: &str,
    runtime: RuntimeMode,
) -> Result<String, Vec<SyntaxError>> {
    let mut parser = Parser::new(source);
    let errors = parser.parse().errors;
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Lua53::new().render(parser.tree(), &EmitCtx { module, runtime }))
}

pub struct EmitDriver {
    emitters: Vec<Box<dyn Emitter>>,
}

impl EmitDriver {
    pub fn new() -> Self {
        EmitDriver { emitters: vec![] }
    }

    pub fn add_emitter(&mut self, emitter: Box<dyn Emitter>) {
        self.emitters.push(emitter);
    }

    pub fn write(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>, target_path: &str) -> bool {
        let mut ok = true;
        for emitter in &self.emitters {
            ok &= emitter.write(ast, ctx, target_path);
        }
        ok
    }
}
