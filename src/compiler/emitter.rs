mod lua53;
pub use lua53::Lua53;

use super::plan::ModulePlan;
use super::utils::DFS;
use crate::parser::ast::*;
use crate::parser::parser::*;

/// 一次发射需要的全部工程级信息。和 `ast` 一样当参数传，不挂在 emitter 上：
/// 同一个 emitter 实例要给多个模块复用，存成字段就得每模块改一次可变状态
pub struct EmitCtx<'a> {
    /// 当前模块名（`import {..} in "pkg.account"` 里那个）
    pub module: &'a str,
    /// linter 算好的本模块发射计划（import 改写点 + 前置声明）
    pub plan: &'a ModulePlan,
    /// 多模块工程：注入共享运行时头、注册 pub 类、import 来的类走 `get`。
    /// 单文件工程不开；再叠上「没有前向引用」时输出与 master 逐字一致
    pub shared: bool,
}

pub trait Emitter: DFS<()> {
    /// 返回 false 表示写出失败（原因由实现自己报过），CLI 据此决定退出码
    fn write(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>, target_path: &str) -> bool;
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
