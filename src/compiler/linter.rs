mod highlight_linter;
mod type_linter;

use super::plan::{ModulePlan, ProjectPlan};
pub use super::utils::{DiagLevel, Diagnostic};
use crate::lexer::type_def::Span;
use crate::parser::ast::*;
use crate::parser::parser::*;
#[allow(unused_imports)]
pub use highlight_linter::{Completion, DefLink, SemToken, TokenKind};

/// 一个 linter 对一个文件的统一产物。诊断、高亮与跳转都从这里出来；不产某类
/// 数据的 linter 留空即可，调用方无需再知道具体实现。
#[derive(Debug)]
pub struct LinterOutput {
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
    pub tokens: Vec<SemToken>,
    pub links: Vec<DefLink>,
    pub completions: Vec<Completion>,
    /// 只有类型 linter 会填：本模块的发射计划（import 改写点 + 前置声明）。
    /// 高亮 linter 留空。计划挂在产物上而不是再加一个 trait 方法，是因为
    /// 驱动本来就是「一次 output 交回全部产物」，多开一条通道反而要驱动再攒一遍
    pub plan: Option<ModulePlan>,
}

impl LinterOutput {
    fn empty(source: &str) -> Self {
        LinterOutput {
            source: source.to_string(),
            diagnostics: Vec::new(),
            tokens: Vec::new(),
            links: Vec::new(),
            completions: Vec::new(),
            plan: None,
        }
    }
}

/// 工程分析器的公共边界。每种 linter 自己拥有并驱动自己的 DFS 上下文：
/// TypeLinter 使用类型 Context，HighlightLinter 使用轻量的 `()`，互不倒灌。
pub trait Linter {
    fn name(&self) -> &str;

    /// BUILD 相位一：所有模块先完成这一钩子，再进入相位二。
    /// 只做「导出」这一步 —— 它不依赖任何别的模块
    fn warm_up(&mut self, _ast: &Tree<NodeSyntax<'_>>, _module: &str) {}

    /// BUILD 相位二：绑 import、填类体。依赖相位一已经把**所有**模块的导出
    /// 登记完，所以不能和 warm_up 挤在一趟里
    fn warm_up_imports(&mut self, _ast: &Tree<NodeSyntax<'_>>, _module: &str) {}

    /// CHECK / OUTPUT 相位：完成单文件遍历并一次性交回该 linter 的全部产物。
    fn output(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        module: &str,
        comments: &[Span],
    ) -> LinterOutput;
}

pub struct LintDriver {
    linters: Vec<Box<dyn Linter>>,
}

/// 工程级检查的完整产物：诊断 + 发射计划
pub struct ProjectCheck {
    pub results: Vec<(String, Vec<Diagnostic>)>,
    pub plan: ProjectPlan,
}

impl LintDriver {
    pub fn new() -> Self {
        LintDriver { linters: vec![] }
    }

    pub fn init<T: Linter + 'static>(mut self, linter: T) -> Self {
        self.linters.push(Box::new(linter));
        self
    }

    /// 工程级检查兼容入口。它仍经过统一的 output_project，只取其中诊断部分。
    pub fn check_project(
        modules: &[(&str, &Tree<NodeSyntax<'_>>)],
    ) -> Vec<(String, Vec<Diagnostic>)> {
        Self::check_project_planned(modules).results
    }

    /// 工程级检查 + 发射计划。compile 用这个，check 只用上面的诊断部分。
    pub fn check_project_planned(modules: &[(&str, &Tree<NodeSyntax<'_>>)]) -> ProjectCheck {
        let inputs: Vec<_> = modules
            .iter()
            .map(|&(module, ast)| (module, ast, &[][..]))
            .collect();
        let mut plan = ProjectPlan::default();
        let mut results = Vec::new();
        for (module, outputs) in Self::output_project(&inputs) {
            let mut diagnostics = Vec::new();
            for output in outputs {
                if let Some(module_plan) = output.plan {
                    plan.insert(module.clone(), module_plan);
                }
                diagnostics.extend(output.diagnostics);
            }
            results.push((module, diagnostics));
        }
        ProjectCheck { results, plan }
    }

    /// 唯一的工程级输出入口。默认流水线装入 highlight + type 两个 linter；先让
    /// 所有模块完成 BUILD，再逐模块输出，跨模块索引和 Session 都不吃文件顺序。
    pub fn output_project(
        modules: &[(&str, &Tree<NodeSyntax<'_>>, &[Span])],
    ) -> Vec<(String, Vec<LinterOutput>)> {
        let mut driver = LintDriver::new()
            .init(highlight_linter::HighlightLinter::new())
            .init(type_linter::TypeLinter::new());
        driver.prefill(modules);
        modules
            .iter()
            .map(|&(module, ast, comments)| {
                (
                    module.to_string(),
                    driver.run_outputs_with_comments(ast, module, comments),
                )
            })
            .collect()
    }

    /// 两相 BUILD：先让所有模块把导出登记完，再让每个模块绑 import、填类体。
    /// 挤成一趟就会露出文件顺序：a 的 import 可能查的是 b 还没登记的导出
    pub fn prefill(&mut self, modules: &[(&str, &Tree<NodeSyntax<'_>>, &[Span])]) {
        for &(module, ast, _) in modules {
            for linter in self.linters.iter_mut() {
                linter.warm_up(ast, module);
            }
        }
        for &(module, ast, _) in modules {
            for linter in self.linters.iter_mut() {
                linter.warm_up_imports(ast, module);
            }
        }
    }

    /// 兼容单 linter 测试与旧调用方：无注释输入，只取诊断。
    pub fn run(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) -> Vec<Diagnostic> {
        self.run_outputs(ast, module)
            .into_iter()
            .flat_map(|output| output.diagnostics)
            .collect()
    }

    pub fn run_outputs(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) -> Vec<LinterOutput> {
        self.run_outputs_with_comments(ast, module, &[])
    }

    fn run_outputs_with_comments(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        module: &str,
        comments: &[Span],
    ) -> Vec<LinterOutput> {
        self.linters
            .iter_mut()
            .map(|linter| linter.output(ast, module, comments))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parser::Parser;

    #[test]
    fn output_project_collects_all_linter_products() {
        let mut parser = Parser::new("-- note\nlocal n: number = 1");
        let comments = parser.parse().comments;
        let modules = [("main", parser.tree(), comments.as_slice())];

        let results = LintDriver::output_project(&modules);
        let outputs = &results[0].1;
        assert_eq!(
            outputs
                .iter()
                .map(|output| output.source.as_str())
                .collect::<Vec<_>>(),
            vec!["Highlight", "Type"]
        );
        assert!(!outputs[0].tokens.is_empty());
        assert!(!outputs[0].completions.is_empty());
        assert!(outputs[0].diagnostics.is_empty());
        assert!(outputs[1].tokens.is_empty());
        assert!(outputs[1].completions.is_empty());
    }
}
