use std::fmt;
// 宏 for 保留字，主要重建 string <-> Reserved 
macro_rules! reserved {
    ($($key_word:ident => $origin_str:literal),+ $(,)?) =>{
        #[derive(Debug,PartialEq,Eq,Clone,Copy)]
        pub enum Reserved { $($key_word),+ }

        impl Reserved { 
            pub const LEN: usize = [$(Reserved::$key_word),+].len();
            pub const RESERVERD_NAMES: [&'static str; Reserved::LEN] = [$(stringify!($key_word)),+];

            pub const fn resolve(self)-> &'static str {
                match self { $(Reserved::$key_word => $origin_str),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($origin_str => Some(Reserved::$key_word),)+ _ => None }
            }
        }
    }
}

// OpType <-> string ，同上
macro_rules! operators {
    ($($op:ident => $origin_str:literal),+ $(,)?) => {
        #[derive(Debug, PartialEq, Eq, Clone, Copy)]
        pub enum OpType { 
            $($op,)+ 
            // 单字符可以直接拿出来
            SIMPLE(char),
        }

        impl OpType {
            pub const COMPLEX_OP_LEN: usize = [$(OpType::$op),+].len();
            pub const COMPLEX_OP_NAMES: [&'static str; OpType::COMPLEX_OP_LEN] = [$(stringify!($op)),+];

            pub const fn index(self) -> Option<usize> {
                let mut i = 0;
                $(
                    if matches!(self, OpType::$op) { return Some(i); }
                    i += 1;
                )+
                let _ = i;
                None
            }

            pub fn resolve(self) -> &'static str {
                match self { 
                    $(
                        OpType::$op => $origin_str,
                    )+
                    //单字符返回空，记得消费时前向拦截
                    OpType::SIMPLE(c) => "",
            }
        }
    }
}
}

reserved! {
    CLASS    => "class",        // +TLUA
    TYPEDEF  => "typedef",      // +TLUA：不用 type，`type(x)` 是 Lua 标准库函数
    AS       => "as",           // +TLUA
    AND      => "and",
    BREAK    => "break",
    DO       => "do",
    ELSE     => "else",
    ELSEIF   => "elseif",
    END      => "end",
    FALSE    => "false",
    FOR      => "for",
    FUNCTION => "function",
    GOTO     => "goto",
    IF       => "if",
    IN       => "in",
    LOCAL    => "local",
    NIL      => "nil",
    NOT      => "not",
    OR       => "or",
    REPEAT   => "repeat",
    RETURN   => "return",
    THEN     => "then",
    TRUE     => "true",
    UNTIL    => "until",
    WHILE    => "while",
}

operators! {
    ELLIPSIS  => "...",
    CONCAT    => "..",
    ARROW     => "->",          // +TLUA：返回类型箭头
    DBCOLON   => "::",
    TURBOFISH => "::<",         // +TLUA：显式类型实参。'<' 在表达式位置是比较运算，
                                //   所以泛型实参不能用裸 '<>'；而 '::' '<' 分开产出
                                //   会和 '::label::' 在 1 个 lookahead 内分不开，
                                //   故 '::<' 整体成 token（要求三字符紧邻）
    EQ        => "==",
    NE        => "~=",
    LE        => "<=",
    GE        => ">=",
    SHL       => "<<",
    SHR       => ">>",
    IDIV      => "//",
}

#[derive(Debug, PartialEq, Clone)]
pub enum Token<'a> {
    STRING(&'a str),
    NUMERAL(&'a str),
    NAME(&'a str),
    RESERVED(Reserved),
    OPERATOR(OpType),
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::STRING(s) => write!(f, "\"{}\"", s),
            Token::NUMERAL(n) => write!(f, "{}", n),
            Token::NAME(n) => write!(f, "{}", n),
            Token::RESERVED(r) => write!(f, "{}", r.resolve()),
            Token::OPERATOR(o) => write!(f, "{}", o.resolve()),
        }
    }
}

#[derive(Debug)]
pub enum AcceptType{
    OPERATOR(OpType),
    STRING,
    COMMENT,    //lexer直接丢弃
    NUMERAL,
    NAME,
}


pub enum Step {
    CONTINUE(State),          //转移到下一状态，继续吃字符
    ACCEPT(AcceptType),     //产出token，状态机已自动复位到S0
    REJECT(LexErr),               //词法错误，状态机已自动复位到S0
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    S0,
    MINUS,
    CMT,
    SCMT,
    LBKT(u32,bool),         //bool: 是否长注释模式
    LSTR(u32,bool),
    LCLOSE(u32,u32,bool),
    DQ,
    SQ,
    DQESC,
    SQESC,
    ZERO,
    HEXI,
    HEXF,
    HEXP,
    HEXPS,
    HEXE,
    INT,
    FRAC,
    E,
    ES,
    EXP,
    DOT,
    DOT2,
    IDENT,
    COLON,
    DCOLON,
    EQ,
    TILDE,
    LT,
    GT,
    SLASH,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum LexErr {
    UnexpectedChar(char),
    UnclosedString,
    UnclosedLongStr,
    UnclosedLongComment,
}

impl fmt::Display for LexErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LexErr::UnexpectedChar(c) => write!(f, "Unexpected character: {}", c),
            LexErr::UnclosedString => write!(f, "Unclosed string"),
            LexErr::UnclosedLongStr => write!(f, "Unclosed long string"),
            LexErr::UnclosedLongComment => write!(f, "Unclosed long comment"),
        }
    }
}

//token/错误的字节区间[start,end)；行列号报错时由LineIndex惰性推导，不在此存储
#[derive(Debug, PartialEq, Clone, Copy, Default )]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

//输入耗尽时的收尾结果：区分"干净EOF"、"中间态补发token"与"未闭合错误"三态
pub enum Finish {
    Done,                       //S0：无待产出token，干净结束
    Token(AcceptType, u32),     //可接受中间态补发token，u32为回退字节数
    Error(LexErr),              //未闭合的字符串/长括号
}