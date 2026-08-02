use super::type_def::*;

pub struct StateMachine {
    current: State,
}

impl StateMachine {
    pub fn new() -> StateMachine {
        StateMachine {
            current: State::S0,
        }
    }

    //hint：lexer回退字节数
    pub fn next_state(&mut self, c: char)-> (Step, u32) {
        let (step, hint): (Step, u32) = match self.current {
            State::S0 => {
                match c {
                    ' ' | '\t' | '\n' | '\r' => (Step::CONTINUE(State::S0), 0),
                    '-' => (Step::CONTINUE(State::MINUS), 0),
                    '[' => (Step::CONTINUE(State::LBKT(0,false)), 0),
                    '"' => (Step::CONTINUE(State::DQ), 0),
                    '\'' => (Step::CONTINUE(State::SQ), 0),
                    '0' => (Step::CONTINUE(State::ZERO), 0),
                    '1'..='9' => (Step::CONTINUE(State::INT), 0),
                    '.' => (Step::CONTINUE(State::DOT), 0),
                    'a'..='z' | 'A'..='Z' | '_' => (Step::CONTINUE(State::IDENT), 0),
                    ':' => (Step::CONTINUE(State::COLON), 0),
                    '=' => (Step::CONTINUE(State::EQ), 0),
                    '~' => (Step::CONTINUE(State::TILDE), 0),
                    '<' => (Step::CONTINUE(State::LT), 0),
                    '>' => (Step::CONTINUE(State::GT), 0),
                    '/' => (Step::CONTINUE(State::SLASH), 0),
                    op @ ('+' | '*' | '%' | '^' | '#' | '&' | '|' | '(' | ')' | '{' | '}'|']'|';'|',' )=> (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE(op))), 0),
                    _ => (Step::REJECT(LexErr::UnexpectedChar(c)), 0),
                }
            },
            State::MINUS => {
                match c {
                    '-' => (Step::CONTINUE(State::CMT), 0),
                    '>' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::ARROW)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('-'))), 1),
                }
            },
            State::CMT => {
                match c {
                    '[' => (Step::CONTINUE(State::LBKT(0,true)), 0),
                    _ => (Step::CONTINUE(State::SCMT), 1),
                }
            },
            State::SCMT => {
                match c {
                    '\n'  => (Step::ACCEPT(AcceptType::COMMENT), 0),
                    _ => (Step::CONTINUE(State::SCMT), 0),
                }
            },
            State::LBKT(n,cmt) =>{
                match c {
                    '=' => (Step::CONTINUE(State::LBKT(n+1,cmt)), 0),
                    '[' => (Step::CONTINUE(State::LSTR(n,cmt)), 0),
                    //非长括号：注释模式退化为短注释；否则accept'['，回退n个'='与当前字符
                    _ => if cmt {(Step::CONTINUE(State::SCMT), 1)}
                         else {(Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('['))), n+1)},
                }
            },
            State::LSTR(n,cmt)=>{
                match c {
                    ']' => (Step::CONTINUE(State::LCLOSE(n,0,cmt)), 0),
                    _ => (Step::CONTINUE(State::LSTR(n,cmt)), 0)
                }
            },
            State::LCLOSE(n,k,cmt)=>{
                match c {
                    '=' => (Step::CONTINUE(State::LCLOSE(n,k+1,cmt)),0),
                    //k数的是'='，新']'是新一轮闭合候选的开头，回到LCLOSE(n,0)而非k+1
                    ']' => if k==n {(Step::ACCEPT(if cmt {AcceptType::COMMENT} else {AcceptType::STRING}),0)}
                           else {(Step::CONTINUE(State::LCLOSE(n,0,cmt)), 0)},
                    _ => (Step::CONTINUE(State::LSTR(n,cmt)), 0)
                }
            },
            State::DQ => {
                match c {
                    '"' => (Step::ACCEPT(AcceptType::STRING), 0),
                    '\\' => (Step::CONTINUE(State::DQESC),0),
                    '\n' => (Step::REJECT(LexErr::UnclosedString), 0),
                    _ => (Step::CONTINUE(State::DQ), 0),
                }
            },
            State::DQESC => {
                match c {
                    //转义后任意字符（含'\n'）回到字符串体
                    _ => (Step::CONTINUE(State::DQ), 0),     
                }
            },
            State::SQ => {
                match c {
                    '\'' => (Step::ACCEPT(AcceptType::STRING), 0),
                    '\\' => (Step::CONTINUE(State::SQESC),0),
                    '\n' => (Step::REJECT(LexErr::UnclosedString), 0),
                    _ => (Step::CONTINUE(State::SQ), 0),
                }
            },
            State::SQESC => {
                match c {
                    _ => (Step::CONTINUE(State::SQ), 0),
                }
            },
            State::ZERO => {
                match c {
                    'x' | 'X' => (Step::CONTINUE(State::HEXI), 0),
                    '0'..='9' => (Step::CONTINUE(State::INT), 0),
                    '.' => (Step::CONTINUE(State::FRAC), 0),
                    'e' | 'E' => (Step::CONTINUE(State::E), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::HEXI => {
                match c {
                    '0'..='9' | 'a'..='f' | 'A'..='F' => (Step::CONTINUE(State::HEXI), 0),
                    '.' => (Step::CONTINUE(State::HEXF), 0),
                    'p' | 'P' => (Step::CONTINUE(State::HEXP), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::HEXF => {
                match c {
                    '0'..='9' | 'a'..='f' | 'A'..='F' => (Step::CONTINUE(State::HEXF), 0),
                    'p' | 'P' => (Step::CONTINUE(State::HEXP), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::HEXP => {
                match c {
                    '+' | '-' => (Step::CONTINUE(State::HEXPS), 0),
                    '0'..='9' => (Step::CONTINUE(State::HEXE), 0),
                    //回退'p'与当前字符，NUMERAL不含'p'
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 2),   
                }
            },
            State::HEXPS => {
                match c {
                    '0'..='9' => (Step::CONTINUE(State::HEXE), 0),
                    //回退'p'、符号与当前字符
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 3),   
                }
            },
            State::HEXE => {
                match c {
                    '0'..='9' => (Step::CONTINUE(State::HEXE), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::INT => {
                match c {
                    '0'..='9' => (Step::CONTINUE(State::INT), 0),
                    '.' => (Step::CONTINUE(State::FRAC), 0),
                    'e' | 'E' => (Step::CONTINUE(State::E), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::FRAC => {
                match c {
                    '0'..='9' => (Step::CONTINUE(State::FRAC), 0),
                    'e' | 'E' => (Step::CONTINUE(State::E), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::E => {
                match c {
                    '+' | '-' => (Step::CONTINUE(State::ES), 0),
                    '0'..='9' => (Step::CONTINUE(State::EXP), 0),
                    //回退'e'与当前字符，NUMERAL不含'e'
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 2),   
                }
            },
            State::ES => {
                match c {
                    '0'..='9' => (Step::CONTINUE(State::EXP), 0),
                    //回退'e'、符号与当前字符
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 3),   
                }
            },
            State::EXP => {
                match c {
                    '0'..='9' => (Step::CONTINUE(State::EXP), 0),
                    _ => (Step::ACCEPT(AcceptType::NUMERAL), 1),
                }
            },
            State::DOT => {
                match c {
                    //形如.5的数字
                    '0'..='9' => (Step::CONTINUE(State::FRAC), 0),      
                    '.' => (Step::CONTINUE(State::DOT2), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('.'))), 1),
                }
            },
            State::DOT2 => {
                match c {
                    '.' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::ELLIPSIS)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::CONCAT)), 1),
                }
            },
            State::IDENT => {
                match c {
                    'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => (Step::CONTINUE(State::IDENT), 0),
                    _ => (Step::ACCEPT(AcceptType::NAME), 1),
                }
            },
            State::COLON => {
                match c {
                    ':' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::DBCOLON)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE(':'))), 1),
                }
            },
            State::EQ => {
                match c {
                    '=' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::EQ)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('='))), 1),
                }
            },
            State::TILDE => {
                match c {
                    '=' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::NE)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('~'))), 1),
                }
            },
            State::LT => {
                match c {
                    '=' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::LE)), 0),
                    '<' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SHL)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('<'))), 1),
                }
            },
            State::GT => {
                match c {
                    //状态机为纯函数，不接收parser反馈：泛型闭合处的'>>'/'>='统一产出SHR/GE，
                    //由parser驱动层在类型上下文拆为'>' '>'与'>' '='
                    '=' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::GE)), 0),
                    '>' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SHR)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('>'))), 1),
                }
            },
            State::SLASH => {
                match c {
                    '/' => (Step::ACCEPT(AcceptType::OPERATOR(OpType::IDIV)), 0),
                    _ => (Step::ACCEPT(AcceptType::OPERATOR(OpType::SIMPLE('/'))), 1),
                }
            },
        };
        //非终态写回current；ACCEPT/REJECT自动复位
        self.current = if let Step::CONTINUE(s) = step { s } else { State::S0 };
        (step, hint)
    }

    //输入耗尽时收尾：等价于给当前状态喂一个other，但该字符不存在，
    //故回退数比正常other分支少1。Token的u32为回退字节数；
    pub fn finish(&mut self) -> Finish {
        let result = match self.current {
            State::S0 => Finish::Done,
            State::MINUS => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('-')), 0),
            State::CMT | State::SCMT => Finish::Token(AcceptType::COMMENT, 0),
            State::LBKT(n,false) => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('[')), n),   //回退n个'='
            State::LBKT(_,true) => Finish::Token(AcceptType::COMMENT, 0),      //"--[=="到EOF：整体是注释
            State::LSTR(_,false) | State::LCLOSE(_,_,false) => Finish::Error(LexErr::UnclosedLongStr),
            State::LSTR(_,true) | State::LCLOSE(_,_,true) => Finish::Error(LexErr::UnclosedLongComment),
            State::DQ | State::SQ | State::DQESC | State::SQESC => Finish::Error(LexErr::UnclosedString),
            State::ZERO | State::HEXI | State::HEXF | State::HEXE
            | State::INT | State::FRAC | State::EXP => Finish::Token(AcceptType::NUMERAL, 0),
            State::HEXP => Finish::Token(AcceptType::NUMERAL, 1),   //回退'p'
            State::HEXPS => Finish::Token(AcceptType::NUMERAL, 2),  //回退'p'和符号
            State::E => Finish::Token(AcceptType::NUMERAL, 1),      //回退'e'
            State::ES => Finish::Token(AcceptType::NUMERAL, 2),     //回退'e'和符号
            State::DOT => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('.')), 0),
            State::DOT2 => Finish::Token(AcceptType::OPERATOR(OpType::CONCAT), 0),
            State::IDENT => Finish::Token(AcceptType::NAME, 0),
            State::COLON => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE(':')), 0),
            State::EQ => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('=')), 0),
            State::TILDE => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('~')), 0),
            State::LT => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('<')), 0),
            State::GT => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('>')), 0),
            State::SLASH => Finish::Token(AcceptType::OPERATOR(OpType::SIMPLE('/')), 0),
        };
        self.current = State::S0;
        result
    }

    pub fn reset(&mut self) {
        self.current = State::S0;
    }
}