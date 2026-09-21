mod highlight_linter;
mod type_linter;

pub use super::utils::{DiagLevel, Diagnostic};
use crate::lexer::type_def::Span;
use crate::parser::ast::*;
use crate::parser::parser::*;
#[allow(unused_imports)]
pub use highlight_linter::{DefLink, SemToken, TokenKind};

/// 一个 linter 对一个文件的统一产物。诊断、高亮与跳转都从这里出来；不产某类
/// 数据的 linter 留空即可，调用方无需再知道具体实现。
#[derive(Debug)]
pub struct LinterOutput {
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
    pub tokens: Vec<SemToken>,
    pub links: Vec<DefLink>,
}

impl LinterOutput {
    fn empty(source: &str) -> Self {
        LinterOutput {
            source: source.to_string(),
            diagnostics: Vec::new(),
            tokens: Vec::new(),
            links: Vec::new(),
        }
    }
}

/// 工程分析器的公共边界。每种 linter 自己拥有并驱动自己的 DFS 上下文：
/// TypeLinter 使用类型 Context，HighlightLinter 使用轻量的 `()`，互不倒灌。
pub trait Linter {
    fn name(&self) -> &str;

    /// BUILD 相位：所有模块先完成这一钩子，再开始任一模块的 output。
    fn warm_up(&mut self, _ast: &Tree<NodeSyntax<'_>>, _module: &str) {}

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
        let inputs: Vec<_> = modules
            .iter()
            .map(|&(module, ast)| (module, ast, &[][..]))
            .collect();
        Self::output_project(&inputs)
            .into_iter()
            .map(|(module, outputs)| {
                let diagnostics = outputs
                    .into_iter()
                    .flat_map(|output| output.diagnostics)
                    .collect();
                (module, diagnostics)
            })
            .collect()
    }

    /// 唯一的工程级输出入口。默认流水线装入 highlight + type 两个 linter；先让
    /// 所有模块完成 BUILD，再逐模块输出，跨模块索引和 Session 都不吃文件顺序。
    pub fn output_project(
        modules: &[(&str, &Tree<NodeSyntax<'_>>, &[Span])],
    ) -> Vec<(String, Vec<LinterOutput>)> {
        let mut driver = LintDriver::new()
            .init(highlight_linter::HighlightLinter::new())
            .init(type_linter::TypeLinter::new());
        for &(module, ast, _) in modules {
            driver.prefill(ast, module);
        }
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

    pub fn prefill(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) {
        for linter in self.linters.iter_mut() {
            linter.warm_up(ast, module);
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
        assert!(outputs[0].diagnostics.is_empty());
        assert!(outputs[1].tokens.is_empty());
    }
}
