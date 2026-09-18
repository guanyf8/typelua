mod type_linter;

use crate::lexer::type_def::Span;
use crate::parser::ast::*;
use crate::parser::parser::*;

pub struct Logger {
    pub span: Span,
    pub msg: String,
}

/// 一个 lint 阶段。四个钩子都给了空实现：一个 linter 只关心其中一两个，
/// 逼着每个实现都写四遍空函数没有意义
pub trait Linter {
    fn name(&self) -> &'static str;

    /// 换文件了。per-file 的状态在这里清。`module` 是当前文件所属模块名，
    /// 跨模块的 pub / import 靠它认「谁导出的、往哪儿导」——只关心类型阶段的
    /// linter 会用到，其余的忽略即可
    fn prepare(&mut self, _ast: &Tree<NodeSyntax<'_>>, _module: &str, _log: &mut Vec<Logger>) {}

    /// 前序：子节点还没走。只适合「进作用域」这类必须先于子树的动作
    fn enter(&mut self, _ast: &Tree<NodeSyntax<'_>>, _node: usize, _log: &mut Vec<Logger>) {}

    /// 后序：子节点都走完了。综合属性的求值该在这里
    fn leave(&mut self, _ast: &Tree<NodeSyntax<'_>>, _node: usize, _log: &mut Vec<Logger>) {}

    /// 整棵树走完。跨节点攒起来的诊断（未初始化、未使用之类）在这里收口
    fn finish(&mut self, _log: &mut Vec<Logger>) {}
}

pub struct LintDriver {
    linters: Vec<Box<dyn Linter>>,
    log: Vec<Logger>,
}

impl LintDriver {
    pub fn new() -> Self {
        LintDriver {
            linters: vec![],
            log: vec![],
        }
    }

    pub fn init<T: Linter + 'static>(mut self, linter: T) -> Self {
        self.linters.push(Box::new(linter));
        self
    }

    /// 跑一棵树。收 `&mut self` 而不是吃掉 self：linter 的粒度是「阶段」
    /// 而不是「一个文件」，同一个实例要被依次驱过工程里各个文件，
    /// TypeLinter 里的 Session 才攒得起来（全局变量、导出类型都靠它跑过
    /// 文件边界）。per-file 的东西归各 linter 自己在 `prepare` 里清。
    ///
    /// 日志按文件取走：留着的话第二个文件会把第一个文件的诊断再报一遍
    pub fn run(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) -> Vec<Logger> {
        for linter in self.linters.iter_mut() {
            linter.prepare(ast, module, &mut self.log);
        }

        if let Some(root) = ast.get_root() {
            self.walk(ast, root);
        }
        for linter in self.linters.iter_mut() {
            linter.finish(&mut self.log);
        }
        std::mem::take(&mut self.log)
    }

    fn walk(&mut self, ast: &Tree<NodeSyntax<'_>>, root: usize) {
        // 非递归dfs
        //   Enter: 刚进入节点，还没处理子节点
        //   Leave: 所有子节点已处理完，即将离开
        enum Phase {
            Enter,
            Leave,
        }
        let mut stack: Vec<(usize, Phase)> = vec![(root, Phase::Enter)];

        while let Some((node, phase)) = stack.pop() {
            match phase {
                Phase::Enter => {
                    // 调用 enter
                    for linter in &mut self.linters {
                        linter.enter(ast, node, &mut self.log);
                    }

                    // 准备处理子节点
                    let children = ast.get_node(node).children.clone();
                    if children.is_empty() {
                        // 没有子节点，直接 leave
                        for linter in &mut self.linters {
                            linter.leave(ast, node, &mut self.log);
                        }
                    } else {
                        // 有子节点：先把 (node, Leave) 压栈，再把所有子节点 (child, Enter) 逆序压栈
                        // 逆序是因为栈是 LIFO，要保证子节点按顺序处理
                        stack.push((node, Phase::Leave));
                        for child in children.into_iter().rev() {
                            stack.push((child, Phase::Enter));
                        }
                    }
                }
                Phase::Leave => {
                    // 调用 leave
                    for linter in &mut self.linters {
                        linter.leave(ast, node, &mut self.log);
                    }
                }
            }
        }
    }
}
