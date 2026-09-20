mod plugin_linter;
mod type_linter;

use super::utils::{DFS, Diagnostic, walk};
use crate::parser::ast::*;
use crate::parser::parser::*;
pub use type_linter::Context;

// 编辑器插件用的独立分析器（高亮 + 跳转）。刻意不走下面的 LintDriver /
// Linter 那套：它和类型检查隔离，自带遍历与作用域，从这里原样透出
#[allow(unused_imports)]
pub use plugin_linter::{
    DefLink, FileAnalysis, PluginAnalyzer, SemToken, TokenKind, analyze_project,
};

pub trait Linter: DFS<Context> {
    fn name(&self) -> &str;

    fn warm_up(
        &mut self,
        _ctx: &mut Context,
        _ast: &Tree<NodeSyntax<'_>>,
        _module: &str,
        _diags: &mut Vec<Diagnostic>,
    ) {
    }
}

pub struct LintDriver {
    linters: Vec<Box<dyn Linter>>,
    diags: Vec<Diagnostic>,
}

impl LintDriver {
    pub fn new() -> Self {
        LintDriver {
            linters: vec![],
            diags: vec![],
        }
    }

    pub fn init<T: Linter + 'static>(mut self, linter: T) -> Self {
        self.linters.push(Box::new(linter));
        self
    }

    /// 工程级检查入口：装好默认流水线（现在只有 TypeLinter；main 那层在
    /// type_linter 私有模块外够不着它，由这里代装），再跑两相位：先对所有
    /// 模块 prefill（BUILD 相位：登记导出 / 填类型体），再逐个 run（CHECK 相位）。
    /// 相位屏障就落在这个顺序上 —— 全部 BUILD 完才开始 CHECK，于是跨模块的
    /// import / pub 与文件顺序无关。诊断按模块返回
    pub fn check_project(
        modules: &[(&str, &Tree<NodeSyntax<'_>>)],
    ) -> Vec<(String, Vec<Diagnostic>)> {
        let mut driver = LintDriver::new().init(type_linter::TypeLinter::new());
        for &(module, ast) in modules {
            driver.prefill(ast, module);
        }
        modules
            .iter()
            .map(|&(module, ast)| (module.to_string(), driver.run(ast, module)))
            .collect()
    }

    pub fn prefill(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) {
        // per-file 上下文：现建现用，warm_up 自己会在头上重置 / 压文件帧
        let mut ctx = Context::new();
        for linter in self.linters.iter_mut() {
            linter.warm_up(&mut ctx, ast, module, &mut self.diags);
        }
    }

    /// 跑一棵树。收 `&mut self` 而不是吃掉 self：linter 的粒度是「阶段」
    /// 而不是「一个文件」，同一个实例要被依次驱过工程里各个文件，
    /// TypeLinter 里的 Session 才攒得起来（全局变量、导出类型都靠它跑过
    /// 文件边界）。per-file 的东西归各 linter 自己在 `prepare` 里清。
    ///
    /// 日志按文件取走：留着的话第二个文件会把第一个文件的诊断再报一遍
    pub fn run(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) -> Vec<Diagnostic> {
        // per-file 上下文只建一次，贯穿 prepare / walk / finish 三个相位。
        // 它是个栓上局部而不是 linter 的字段：同一个 linter 实例要跨文件复用，
        // 而 per-file 的临时状态不该跟着实例跑
        let mut ctx = Context::new();
        for linter in self.linters.iter_mut() {
            linter.prepare(&mut ctx, ast, module, &mut self.diags);
        }

        // 遍历本身交给公共的 utils::walk：每个 linter 各走一趟（当前管线只一个）。
        // 从「单趟锁步驱多个 linter」改成「逐 linter 各遍历」——各 linter 相互独立、
        // 只经 diags 交流，先后无差
        if let Some(root) = ast.get_root() {
            for linter in self.linters.iter_mut() {
                walk(linter.as_mut(), &mut ctx, ast, root, &mut self.diags);
            }
        }
        for linter in self.linters.iter_mut() {
            linter.finish(&mut ctx, &mut self.diags);
        }
        std::mem::take(&mut self.diags)
    }
}
