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
            "typedef" => Some(Reserved::TYPEDEF),
            "as" => Some(Reserved::AS),
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
                            //注释丢弃后继续扫描
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
                    AcceptType::COMMENT => None,    //末尾注释直接丢弃
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

#[cfg(test)]
mod tests {
    use super::*;

    //既有测试只关心token序列：丢弃span并unwrap掉Result，遇Err直接panic暴露
    fn tok<'a>(r: Option<(Result<Token<'a>, LexErr>, Span)>) -> Option<Token<'a>> {
        r.map(|(x, _)| x.unwrap())
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
        let mut lx = Lexer::new("a->b ... .. :: ::< == ~= <= >= << >> //");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("a")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::ARROW)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("b")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::ELLIPSIS)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::CONCAT)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::DBCOLON)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::TURBOFISH)));
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
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("a")), Span { start: 0, end: 1 })));
        assert_eq!(lx.next_token(), Some((Err(LexErr::UnexpectedChar('?')), Span { start: 2, end: 3 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("b")), Span { start: 4, end: 5 })));   //越过肇事字符后可继续
        assert_eq!(lx.next_token(), None);
    }

    #[test]
    fn unclosed_string_newline_then_continue() {
        let mut lx = Lexer::new("\"abc\nx");
        //span指向自起始引号的整段(不含换行)，报错可定位到未闭合的引号
        assert_eq!(lx.next_token(), Some((Err(LexErr::UnclosedString), Span { start: 0, end: 4 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("x")), Span { start: 5, end: 6 })));
        assert_eq!(lx.next_token(), None);
    }

    #[test]
    fn unclosed_string_eof() {
        let mut lx = Lexer::new("'abc");
        assert_eq!(lx.next_token(), Some((Err(LexErr::UnclosedString), Span { start: 0, end: 4 })));
        assert_eq!(lx.next_token(), None);      //错误后position已到末尾，干净EOF
        //转义后截断同属未闭合
        let mut lx = Lexer::new("\"ab\\");
        assert_eq!(lx.next_token(), Some((Err(LexErr::UnclosedString), Span { start: 0, end: 4 })));
    }

    #[test]
    fn unclosed_long_bracket_distinguishes_string_and_comment() {
        let mut lx = Lexer::new("[[abc");
        assert_eq!(lx.next_token(), Some((Err(LexErr::UnclosedLongStr), Span { start: 0, end: 5 })));
        let mut lx = Lexer::new("--[==[abc]=]");    //级别不匹配，未闭合
        assert_eq!(lx.next_token(), Some((Err(LexErr::UnclosedLongComment), Span { start: 0, end: 12 })));
    }

    #[test]
    fn spans_cover_exact_bytes() {
        //覆盖：关键字/回退token(0x1f后靠空格收尾)/跳过注释后的起点/SHR两字节(供拆分算术)
        let mut lx = Lexer::new("local ab = 0x1f --c\n>>=");
        assert_eq!(lx.next_token(), Some((Ok(Token::RESERVED(Reserved::LOCAL)), Span { start: 0, end: 5 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("ab")), Span { start: 6, end: 8 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::OPERATOR(OpType::SIMPLE('='))), Span { start: 9, end: 10 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::NUMERAL("0x1f")), Span { start: 11, end: 15 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::OPERATOR(OpType::SHR)), Span { start: 20, end: 22 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::OPERATOR(OpType::SIMPLE('='))), Span { start: 22, end: 23 })));
        assert_eq!(lx.next_token(), None);
    }

    #[test]
    fn typedef_and_as_are_keywords() {
        let mut lx = Lexer::new("typedef Id = number  x as T");
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::TYPEDEF)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("Id")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('='))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("number")));   //内建类型名只是普通NAME
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("x")));
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::AS)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("T")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn keyword_match_is_whole_ident_not_prefix() {
        //关键字表在IDENT终结后整体查表，前缀不算命中：as/typedef尤其短，容易误判
        let mut lx = Lexer::new("asa astute typedefs _as self init");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("asa")));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("astute")));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("typedefs")));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("_as")));
        //self是普通参数名、init是保留的方法名，都不是关键字
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("self")));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("init")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn turbofish_is_one_token() {
        let mut lx = Lexer::new("map::<string,number>(xs)");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("map")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::TURBOFISH)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("string")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(','))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("number")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('>'))));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('('))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("xs")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(')'))));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn turbofish_requires_three_chars_adjacent() {
        //'::<'是最大匹配的整体token，中间有空白就退化成DBCOLON + '<'
        let mut lx = Lexer::new("a:: <b");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("a")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::DBCOLON)));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('<'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("b")));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn goto_label_after_call_is_unaffected_by_turbofish() {
        //回归：这是Lua里goto-continue的标准写法，'::'后面必然跟NAME，
        //所以多看一个字符不会把它误吃成turbofish
        let mut lx = Lexer::new("doit(i)\n::continue::");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("doit")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('('))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("i")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(')'))));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::DBCOLON)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("continue")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::DBCOLON)));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn dbcolon_at_eof_still_finishes() {
        //DCOLON是新增的中间态：输入在'::'后耗尽时必须补发DBCOLON，不能丢
        let mut lx = Lexer::new("a::");
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("a")), Span { start: 0, end: 1 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::OPERATOR(OpType::DBCOLON)), Span { start: 1, end: 3 })));
        assert_eq!(lx.next_token(), None);
        //单个':'在EOF的老行为不受影响
        let mut lx = Lexer::new("a:");
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("a")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(':'))));
        assert_eq!(tok(lx.next_token()), None);
    }

    #[test]
    fn turbofish_spans_three_bytes() {
        //span要精确覆盖'::<'，因为报错定位和printer都依赖它
        let mut lx = Lexer::new("map::<T>");
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("map")), Span { start: 0, end: 3 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::OPERATOR(OpType::TURBOFISH)), Span { start: 3, end: 6 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::NAME("T")), Span { start: 6, end: 7 })));
        assert_eq!(lx.next_token(), Some((Ok(Token::OPERATOR(OpType::SIMPLE('>'))), Span { start: 7, end: 8 })));
        assert_eq!(lx.next_token(), None);
    }

    #[test]
    fn turbofish_with_nested_generic_still_emits_shr() {
        //turbofish让泛型进了表达式位置，闭合处照样产出SHR，交给parser驱动层按状态拆分
        let mut lx = Lexer::new("local zs = map::<string,list<number>>(xs)");
        assert_eq!(tok(lx.next_token()), Some(Token::RESERVED(Reserved::LOCAL)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("zs")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('='))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("map")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::TURBOFISH)));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("string")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(','))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("list")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('<'))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("number")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SHR)));    //不拆分
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE('('))));
        assert_eq!(tok(lx.next_token()), Some(Token::NAME("xs")));
        assert_eq!(tok(lx.next_token()), Some(Token::OPERATOR(OpType::SIMPLE(')'))));
        assert_eq!(tok(lx.next_token()), None);
    }
}
