use super::type_def::*;
use super::state_machine::StateMachine;

pub struct Lexer<'a> {
    input: &'a str,
    state_machine: StateMachine,
    position: usize,        //字节偏移
    //跳过的注释区间，按源码顺序。注释不作为token产出（文法里没有COMMENT终结符，
    //产出了parser只能再过滤一遍），但也不能丢：重新生成时要把它们抄回去。
    //和节点的span配合使用——某个节点覆盖[a,b)，落在其中的注释就属于它，
    //因为这个表是有序的，按位置二分即可，不需要把注释挂到每个节点上
    comments: Vec<Span>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Lexer<'a> {
        Lexer {
            input,
            state_machine: StateMachine::new(),
            position: 0,
            comments: vec![],
        }
    }

    /// 扫描过程中跳过的全部注释，按源码顺序。`next_token` 返回 `None` 之后才是完整的
    pub fn comments(&self) -> &[Span] {
        &self.comments
    }

    fn make_token(&self, acc: AcceptType, start: usize, end: usize) -> Option<Token<'a>> {
        match acc {
            AcceptType::OPERATOR(op) => Some(Token::OPERATOR(op)), 
            AcceptType::STRING => Some(Token::STRING(&self.input[start..end])),
            AcceptType::NUMERAL => Some(Token::NUMERAL(&self.input[start..end])),
            AcceptType::NAME => {
                let name = &self.input[start..end];
                match Reserved::parse(name) {
                    Some(r) => Some(Token::RESERVED(r)),
                    None => Some(Token::NAME(name)),
                }
            },
            AcceptType::COMMENT => None,    //不会走到这里
        }
    }
}

impl<'a> Lexer<'a> {
    //None=EOF；token与错误共用span位：错误的span指向肇事区间（未闭合=自token起点）
    pub fn next_token(&mut self) -> Option<(Result<Token<'a>, LexErr>, Span)> {
        let bytes = self.input.as_bytes();
        let mut current = self.position;
        let mut token_start = current;  
        self.state_machine.reset();
        while current < bytes.len() {
            let c = bytes[current] as char;
            let (step, hint) = self.state_machine.next_state(c);
            match step {
                Step::CONTINUE(s) => {
                    current = current + 1 - hint as usize;
                    if let State::S0 = s {
                        token_start = current;      //跳空白，token尚未开始
                    }
                },
                Step::ACCEPT(acc) => {
                    let end = current + 1 - hint as usize;  //token结束(不含回退字符)
                    self.position = end;
                    match acc {
                        AcceptType::COMMENT => {
                            //注释不产出token，但记下区间后继续扫描。
                            //短注释的区间含行尾换行（SCMT 遇 '\n' 时 hint 为 0），
                            //长注释止于 ']]'
                            self.comments.push(Span { start: token_start, end });
                            current = end;
                            token_start = end;
                        },
                        _ => return self.make_token(acc, token_start, end)
                                 .map(|t| (Ok(t), Span { start: token_start, end })),
                    }
                },
                Step::REJECT(e) => {
                    //越过reject字符，lexer永不卡死；调用方可选择继续收集后续错误或fail-fast
                    self.position = current + 1;
                    let span = match e {
                        LexErr::UnexpectedChar(_) => Span { start: current, end: current + 1 },
                        _ => Span { start: token_start, end: current },  //未闭合：指向起始定界符起的整段
                    };
                    return Some((Err(e), span));
                },
            }
        }
        //输入耗尽：状态机可能停在中间态
        match self.state_machine.finish() {
            Finish::Done => {
                self.position = bytes.len();
                None
            },
            Finish::Token(acc, backup) => {
                let end = bytes.len() - backup as usize;
                self.position = end;
                match acc {
                    //末尾注释：后面没有token了，同样记下来
                    AcceptType::COMMENT => {
                        self.comments.push(Span { start: token_start, end });
                        None
                    }
                    _ => self.make_token(acc, token_start, end)
                             .map(|t| (Ok(t), Span { start: token_start, end })),
                }
            },
            Finish::Error(e) => {
                self.position = bytes.len();
                Some((Err(e), Span { start: token_start, end: bytes.len() }))
            },
        }
    }
}
