mod type_linter;
mod types;

use crate::lexer::type_def::Span;
use crate::parser::ast::*;
use crate::parser::parser::*;

pub struct Logger {
    pub span: Span,
    pub msg: String
}

pub trait Linter {
    fn name(&self)->& 'static str;

    fn prepare(&mut self, ast:&Tree<NodeSyntax<'_>>, log:&mut Vec<Logger>){}

    fn enter(&mut self, ast: &Tree<NodeSyntax<'_>>, node:usize, log:&mut Vec<Logger>){}

    fn leave(&mut self, ast: &Tree<NodeSyntax<'_>>, node:usize, log:&mut Vec<Logger>){}

    fn finish(&mut self, log:&mut Vec<Logger>){}
}

pub struct LintDriver{
    linters: Vec<Box<dyn Linter>>,
    log: Vec<Logger>,
}

impl LintDriver{
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

    pub fn run(mut self, ast:&Tree<NodeSyntax<'_>>) -> Vec<Logger> {
        for linter in self.linters.iter_mut() {
            linter.prepare(ast, &mut self.log);
        }
        
        if let Some(root) = ast.get_root() {
            self.walk(ast, root);
        }
        for linter in self.linters.iter_mut() {
            linter.finish(&mut self.log);
        }
        self.log
    }
    
    fn walk(&mut self, ast: &Tree<NodeSyntax<'_>>, root: usize) {
        // 非递归dfs
        //   Enter: 刚进入节点，还没处理子节点
        //   Leave: 所有子节点已处理完，即将离开
        enum Phase { Enter, Leave }
        let mut stack:Vec<(usize, Phase)> = vec![(root, Phase::Enter)];

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