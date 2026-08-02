use super::type_def::*;
use super::state_machine::StateMachine;

pub struct Lexer<'a> {
    input: &'a str,
    state_machine: StateMachine,
    position: usize,        //字节偏移
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Lexer<'a> {
        Lexer {
            input,
            state_machine: StateMachine::new(),
            position: 0,
        }
    }

    //IDENT终结后查关键字表
    fn keyword(name: &str) -> Option<Reserved> {
        match name {
            "class" => Some(Reserved::CLASS),
            "extends" => Some(Reserved::EXTENDS),
            "and" => Some(Reserved::AND),
            "break" => Some(Reserved::BREAK),
            "do" => Some(Reserved::DO),
            "else" => Some(Reserved::ELSE),
            "elseif" => Some(Reserved::ELSEIF),
            "end" => Some(Reserved::END),
            "false" => Some(Reserved::FALSE),
            "for" => Some(Reserved::FOR),
            "function" => Some(Reserved::FUNCTION),
            "goto" => Some(Reserved::GOTO),
            "if" => Some(Reserved::IF),
            "in" => Some(Reserved::IN),
            "local" => Some(Reserved::LOCAL),
            "nil" => Some(Reserved::NIL),
            "not" => Some(Reserved::NOT),
            "or" => Some(Reserved::OR),
            "repeat" => Some(Reserved::REPEAT),
            "return" => Some(Reserved::RETURN),
            "then" => Some(Reserved::THEN),
            "true" => Some(Reserved::TRUE),
            "until" => Some(Reserved::UNTIL),
            "while" => Some(Reserved::WHILE),
            _ => None,
        }
    }

    fn make_token(&self, acc: AcceptType, start: usize, end: usize) -> Option<Token<'a>> {
        match acc {
            AcceptType::OPERATOR(op) => Some(Token::OPERATOR(op)), 
            AcceptType::STRING => Some(Token::STRING(&self.input[start..end])),
            AcceptType::NUMERAL => Some(Token::NUMERAL(&self.input[start..end])),
            AcceptType::NAME => {
                let name = &self.input[start..end];
                match Self::keyword(name) {
                    Some(r) => Some(Token::RESERVED(r)),
                    None => Some(Token::NAME(name)),
                }
            },
            AcceptType::COMMENT => None,    //不会走到这里
        }
    }
}

impl<'a> Lexer<'a> {
    //None=EOF
    pub fn next_token(&mut self) -> Option<Result<Token<'a>, LexErr>> {
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
                            //注释丢弃后继续扫描
                            current = end;
                            token_start = end;
                        },
                        _ => return self.make_token(acc, token_start, end).map(Ok),
                    }
                },
                Step::REJECT(e) => {
                    //越过reject字符，lexer永不卡死；调用方可选择继续收集后续错误或fail-fast
                    self.position = current + 1;
                    return Some(Err(e));
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
                    AcceptType::COMMENT => None,    //末尾注释直接丢弃
                    _ => self.make_token(acc, token_start, end).map(Ok),
                }
            },
            Finish::Error(e) => {
                self.position = bytes.len();
                Some(Err(e))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    //既有测试只关心token序列：unwrap掉Result，遇Err直接panic暴露
    fn tok<'a>(r: Option<Result<Token<'a>, LexErr>>) -> Option<Token<'a>> {
        r.map(|x| x.unwrap())
    }

    #[test]
    fn keywords_names_numbers_comment() {
        let mut lx = Lexer::new("local a = 1 --[[c]] + 0x1f\n");
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::LOCAL)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("a")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('='))));
        assert_eq!(tok(lx.next_token()), Some(Token::NUMERAL("1")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('+'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NUMERAL("0x1f")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn multi_char_operators() {
        let mut lx = Lexer::new("a->b ... .. :: == ~= <= >= << >> //");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("a")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::ARROW)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("b")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::ELLIPSIS)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::CONCAT)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::DBCOLON)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::EQ)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::NE)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::LE)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::GE)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SHL)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SHR)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::IDIV)));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn strings_and_long_brackets() {
        let mut lx = Lexer::new("\"hi\\n\" '' [[raw]] [==[a]=]b]==]");
        assert_eq!(tok(lx.next_token()), Some(Token::STRING("\"hi\\n\"")));
        assert_eq!(tok(lx.next_token()), Some(Token::STRING("''")));
        assert_eq!(tok(lx.next_token()), Some(Token::STRING("[[raw]]")));
        assert_eq!(tok(lx.next_token()), Some(Token::STRING("[==[a]=]b]==]")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn lbkt_rollback_and_comments() {
        //t[=1]: LBKT开启失败需回退n+1个字符
        let mut lx = Lexer::new("t[=1] -- tail comment");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("t")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('['))));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('='))));
        assert_eq!(tok(lx.next_token()), Some(Token::NUMERAL("1")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(']'))));
        assert_eq!(tok(lx.next_token()), None);      //行尾短注释到EOF被丢弃
    }

    #[test]
    fn eof_finishes_pending_token() {
        let mut lx = Lexer::new("local a");
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::LOCAL)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("a")));    //EOF时IDENT收尾
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn eof_exponent_rollback() {
        //"1e"到EOF：NUMERAL("1")，'e'重新词法化为NAME
        let mut lx = Lexer::new("1e");
        assert_eq!(tok(lx.next_token()), Some(Token::NUMERAL("1")));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("e")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn generic_close_emits_ge_for_driver_split() {
        //约定：lexer为纯函数，泛型闭合紧跟赋值时照常产出GE，由parser驱动层拆为'>' '='
        let mut lx = Lexer::new("local t:table<string,any>=init()");
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::LOCAL)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("t")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(':'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("table")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('<'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("string")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(','))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("any")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::GE)));    //不拆分
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("init")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('('))));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(')'))));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn nested_generic_close_emits_shr_for_driver_split() {
        //约定：嵌套泛型闭合'>>='产出SHR '='，由parser驱动层把SHR拆为'>' '>'
        let mut lx = Lexer::new("local x:T<U<V>>=t");
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::LOCAL)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("x")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(':'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("T")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('<'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("U")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('<'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("V")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SHR)));   //不拆分
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('='))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("t")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn unexpected_char_reports_error_and_continues() {
        let mut lx = Lexer::new("a ? b");
        assert_eq!(lx.next_token(), Some(Ok(Token::NAME("a"))));
        assert_eq!(lx.next_token(), Some(Err(LexErr::UnexpectedChar('?'))));
        assert_eq!(lx.next_token(), Some(Ok(Token::NAME("b"))));   //越过肇事字符后可继续
        assert_eq!(lx.next_token(), None);
    }

    #[test]
    fn unclosed_string_newline_then_continue() {
        let mut lx = Lexer::new("\"abc\nx");
        assert_eq!(lx.next_token(), Some(Err(LexErr::UnclosedString)));
        assert_eq!(lx.next_token(), Some(Ok(Token::NAME("x"))));   //越过换行后从下一行继续
        assert_eq!(lx.next_token(), None);
    }

    #[test]
    fn unclosed_string_eof() {
        let mut lx = Lexer::new("'abc");
        assert_eq!(lx.next_token(), Some(Err(LexErr::UnclosedString)));
        assert_eq!(lx.next_token(), None);      //错误后position已到末尾，干净EOF
        //转义后截断同属未闭合
        let mut lx = Lexer::new("\"ab\\");
        assert_eq!(lx.next_token(), Some(Err(LexErr::UnclosedString)));
    }

    #[test]
    fn unclosed_long_bracket_distinguishes_string_and_comment() {
        let mut lx = Lexer::new("[[abc");
        assert_eq!(lx.next_token(), Some(Err(LexErr::UnclosedLongStr)));
        let mut lx = Lexer::new("--[==[abc]=]");    //级别不匹配，未闭合
        assert_eq!(lx.next_token(), Some(Err(LexErr::UnclosedLongComment)));
    }
}