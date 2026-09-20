use core::panic;

use super::adapter::{COL_GT, col};
use super::ast::*;
use super::table::*;
use crate::lexer::lexer::*;
use crate::lexer::type_def::*;

#[derive(Clone)]
pub enum NodeSyntax<'a> {
    Prod(Option<Prod>),
    Token(Token<'a>),
}

impl NodeData for NodeSyntax<'_> {
    fn default() -> Self {
        NodeSyntax::Prod(None)
    }
}

/// 一条语法诊断：字节区间 + 人读信息。词法错误（`next_terminal` 跳过坏字符时）
/// 与文法错误（`Action::Error`）共用这个壳，`main` 按模块名逐条报。
pub struct SyntaxError {
    pub span: Span,
    pub msg: String,
}

/// `parse` 的完整产物。注释与错误**分列两处** —— 从前 `parse` 只返回注释、
/// 却被 `main` 当错误报（源里每条注释都成了「语法错误」），这里把两者彻底分开。
/// 出错时 `tree` 可能不完整；调用方据 `errors` 非空跳过后续阶段，不来读这棵半成品。
pub struct Parsed<'p, 'a> {
    pub tree: &'p Tree<NodeSyntax<'a>>,
    pub comments: Vec<Span>,
    pub errors: Vec<SyntaxError>,
}

pub struct Parser<'a> {
    node_stack: Vec<usize>, //指示node的指针
    ast: Tree<NodeSyntax<'a>>,
    input: &'a str,
    //把 '>>' / '>=' 拆开之后，后半个 token 存在这里等下一次取
    pending: Option<(Token<'a>, Span)>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        Parser {
            node_stack: vec![],
            ast: Tree::new(),
            input,
            pending: None,
        }
    }

    /// 已建好的语法树，原样借出。并行解析时 `parse()` 返回的 `&Tree` 借着
    /// Parser、跨不了线程边界，只能先在各自线程里建好树、join 后再回来
    /// 按着这个口取，免得重跑一遍 parse。未 parse 过时拿到的是刚初始化的空树
    pub fn tree(&self) -> &Tree<NodeSyntax<'a>> {
        &self.ast
    }

    //传state是为了判断当前是否 >> 或 >=
    fn next_terminal(
        &mut self,
        lexer: &mut Lexer<'a>,
        state: u32,
        errors: &mut Vec<SyntaxError>,
    ) -> Option<(Token<'a>, Span)> {
        //上一次拆出来的后半个优先。它已经是单字符，不可能再需要拆
        if let Some(half) = self.pending.take() {
            return Some(half);
        }

        //词法错误不再中断：记一条诊断，跳过坏区间接着取。lexer 每次至少前进一个
        //字符，循环必然收敛到 EOF（None），不会卡死
        let (token, span) = loop {
            match lexer.next_token()? {
                (Ok(token), span) => break (token, span),
                (Err(e), span) => errors.push(SyntaxError {
                    span,
                    msg: format!("词法错误：{e}"),
                }),
            }
        };

        //泛型闭合处的 '>>' / '>=' 拆回单个 '>'，当前parser状态下：
        //    lookahead='>' 动作，而 SHR/GE 是 error  ->  拆
        //    两者都有动作                    ->  不拆
        let second = match token {
            Token::OPERATOR(OpType::SHR) => Some(OpType::SIMPLE('>')),
            Token::OPERATOR(OpType::GE) => Some(OpType::SIMPLE('=')),
            _ => None,
        };
        if let Some(second) = second {
            let gt_legal = action(state, COL_GT) != Action::Error;
            let whole_legal = action(state, col(&token)) != Action::Error;
            if gt_legal && !whole_legal {
                //span 也要切成两半，否则 printer 会把 '>=' 打两遍
                let mid = span.start + 1;
                self.pending = Some((
                    Token::OPERATOR(second),
                    Span {
                        start: mid,
                        end: span.end,
                    },
                ));
                return Some((
                    Token::OPERATOR(OpType::SIMPLE('>')),
                    Span {
                        start: span.start,
                        end: mid,
                    },
                ));
            }
        }
        Some((token, span))
    }

    //build tree
    pub fn parse(&mut self) -> Parsed<'_, 'a> {
        let mut lexer = Lexer::new(self.input);
        let mut state_stack: Vec<u32> = vec![0]; // state 0 初始化
        let mut errors: Vec<SyntaxError> = Vec::new();

        let mut lookahead = self.next_terminal(&mut lexer, 0, &mut errors);

        loop {
            let symbol_col = match &lookahead {
                Some((token, _span)) => {
                    let col = col(token);
                    assert_ne!(col, NOT_EXIST, "{} 不是本文法的终结符", token);
                    col
                }
                None => END_SYMBOL,
            };

            match action(*state_stack.last().unwrap(), symbol_col) {
                Action::Shift(state) => {
                    state_stack.push(state);

                    let (token, span) = lookahead.take().unwrap();
                    let (node, index) = self.ast.alloc_node();
                    node.set_data(NodeSyntax::Token(token));
                    node.set_span(span);
                    self.node_stack.push(index);

                    // 仅shift才更新输入
                    lookahead = self.next_terminal(&mut lexer, state, &mut errors);
                }
                Action::Reduce(rule) => {
                    let rule_len = RULE_RHS_LEN[rule as usize];
                    let prod_rule = prod(rule);

                    //动栈
                    state_stack.truncate(state_stack.len() - rule_len as usize);
                    let rule_name = RULE_LHS[rule as usize];
                    let next_state = goto(*state_stack.last().unwrap(), rule_name).expect(
                        "No such GOTO({*state_stack.last().unwrap(): usize}, {rule_name: String})",
                    );
                    state_stack.push(next_state);

                    // 小优化
                    // 无标签 + 恰好一个子节点 = 纯透传：复用 node_stack 顶上结果，
                    // 不新建节点。它的 span 和唯一的子节点逐字节相同。
                    //
                    // 空产生式不可折叠
                    if prod_rule.is_none() && rule_len == 1 {
                        // 纯透传，什么都不做
                    } else {
                        let children = self
                            .node_stack
                            .split_off(self.node_stack.len() - rule_len as usize);
                        let span = match (children.first(), children.last()) {
                            // 有子节点
                            (Some(&first), Some(&last)) => Span {
                                start: self.ast.span_of(first).start,
                                end: self.ast.span_of(last).end,
                            },
                            // ε 产生式没有child，span 推不出来，取当前 lookahead 的起点。
                            _ => {
                                let at = lookahead
                                    .as_ref()
                                    .map_or(self.input.len(), |(_, span)| span.start);
                                Span { start: at, end: at }
                            }
                        };
                        let (node, index) = self.ast.alloc_node();
                        node.set_data(NodeSyntax::Prod(prod_rule));
                        node.set_span(span);
                        for child in children {
                            self.ast.relation(index, child);
                        }
                        self.node_stack.push(index);
                    }
                }
                Action::Accept => {
                    break;
                }
                Action::Error => {
                    //不再 panic：记一条错误就地收尾。走 fail-fast —— 报第一个错、
                    //位置最准；错误恢复（跳到同步符接着报）留到确实要多错时再做。
                    let mut expected = expected_terminals(*state_stack.last().unwrap());
                    expected.sort_unstable();
                    let (span, msg) = match &lookahead {
                        Some((token, span)) => {
                            (*span, format!("非预期的 {token}，期望 {expected:?} 之一"))
                        }
                        None => {
                            let at = self.input.len();
                            (
                                Span { start: at, end: at },
                                format!("输入意外结束，期望 {expected:?} 之一"),
                            )
                        }
                    };
                    errors.push(SyntaxError { span, msg });
                    break;
                }
            }
        }

        // 正常收尾：栈顶就是根。出错时栈未必归一，这时别强求根 —— 调用方看
        // errors 非空会跳过后续阶段，不会来读这棵半成品
        if self.node_stack.len() == 1 {
            let node = self.node_stack.pop().unwrap();
            self.ast.set_root(node);
        } else if errors.is_empty() {
            panic!("Invalid AST, stack length should be 1");
        }

        Parsed {
            tree: &self.ast,
            comments: lexer.comments().to_vec(),
            errors,
        }
    }
}
