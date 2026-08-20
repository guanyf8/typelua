use core::panic;

use super::table::*;
use super::ast::*;
use crate::lexer::lexer::*;
use crate::lexer::type_def::*;
use super::adapter::{COL_GT, col};

#[derive(Clone)]
enum NodeSyntax<'a> {
    Prod(Option<Prod>),
    Token(Token<'a>),
}

impl NodeData for NodeSyntax<'_> {
    fn default() -> Self {
        NodeSyntax::Prod(None)
    }
}

struct parser<'a> {
    node_stack: Vec<usize>,  //指示node的指针
    ast: Tree<NodeSyntax<'a>>,
    input: &'a str,
    //把 '>>' / '>=' 拆开之后，后半个 token 存在这里等下一次取
    pending: Option<(Token<'a>, Span)>,
}

impl<'a> parser<'a> {
    fn new(input: &'a str) -> Self {
        parser {
            node_stack: vec![],
            ast: Tree::new(),
            input,
            pending: None,
        }
    }

    //传state是为了判断当前是否 >> 或 >=
    fn next_terminal(&mut self, lexer: &mut Lexer<'a>, state: u32)
                    -> Option<(Token<'a>, Span)> {
        //上一次拆出来的后半个优先。它已经是单字符，不可能再需要拆
        if let Some(half) = self.pending.take() {
            return Some(half);
        }

        let (token, span) = match lexer.next_token()? {
            (Ok(token), span) => (token, span),
            (Err(e), span) => panic!("lexical error at {}..{}: {e}", span.start, span.end),
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
                    Span { start: mid, end: span.end },
                ));
                return Some((
                    Token::OPERATOR(OpType::SIMPLE('>')),
                    Span { start: span.start, end: mid },
                ));
            }
        }
        Some((token, span))
    }

    //build tree
    pub fn parse(&mut self) {
        let mut lexer = Lexer::new(self.input);
        let mut state_stack: Vec<u32> = vec![0];  // state 0 初始化
        
        let mut lookahead = self.next_terminal(&mut lexer, 0);

        loop {
            let symbol_col = match &lookahead {
                Some((token, _span)) => {
                    let col=col(token);
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
                    lookahead=self.next_terminal(&mut lexer, state);
                },
                Action::Reduce(rule) => {
                    let rule_len = RULE_RHS_LEN[rule as usize];
                    let prod_rule = prod(rule);

                    //动栈
                    state_stack.truncate(state_stack.len() - rule_len as usize);
                    let rule_name = RULE_LHS[rule as usize];
                    let next_state = goto(*state_stack.last().unwrap(), rule_name)
                                            .expect("No such GOTO({*state_stack.last().unwrap(): usize}, {rule_name: String})");
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
                },
                Action::Accept => {
                    break;
                },
                Action::Error => {
                    let mut expected = expected_terminals(*state_stack.last().unwrap());
                    expected.sort_unstable();
                    match &lookahead {
                        Some((token, span)) => panic!(
                            "syntax error at {}..{}: unexpected {token:?}, expected one of {expected:?}",
                            span.start, span.end),
                        None => panic!("syntax error at end of input: expected one of {expected:?}"),
                    }
                },
            }


        }

        // 此时栈顶节点即为根节点
        if self.node_stack.len() != 1 {
            panic!("Invalid AST, stack length should be 1");
        }else{
            let node = self.node_stack.pop().unwrap();
            self.ast.set_root(node);
        }

    }
}
