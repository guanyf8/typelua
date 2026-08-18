
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum OpType {
    ELLIPSIS,   //...
    CONCAT,     //..
    ARROW,      // ->   in typelua
    DBCOLON,    //::
    TURBOFISH,  // ::<  in typelua：显式类型实参。'<'在表达式位置是比较运算，
                //      所以泛型实参不能用裸'<>'；而'::' '<'分开产出会和'::label::'
                //      在1个lookahead内分不开，故'::<'整体成token（要求三字符紧邻）
    EQ,         //==
    NE,         //~=
    LE,         //<=
    GE,         //>=
    SHL,        //<<
    SHR,        //>>
    IDIV,       // '//'
    SIMPLE(char) 
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Reserved {
    CLASS,    //class in typelua
    EXTENDS,  //extends in typelua
    TYPEDEF,  //typedef in typelua：类型声明。不用type是因为type(x)是Lua标准库函数
    AS,       //as in typelua：强转
    AND,      //and 
    BREAK,    //break
    DO,       //do
    ELSE,     //else
    ELSEIF,   //elseif
    END,      //end
    FALSE,    //false
    FOR,      //for
    FUNCTION, //function
    GOTO,     //goto
    IF,       //if
    IN,       //in
    LOCAL,    //local
    NIL,      //nil
    NOT,      //not
    OR,       //or
    REPEAT,   //repeat
    RETURN,   //return
    THEN,     //then
    TRUE,     //true
    UNTIL,    //until
    WHILE,    //while
}

#[derive(Debug, PartialEq, Clone)]
pub enum Token<'a> {
    STRING(&'a str),
    NUMERAL(&'a str),
    NAME(&'a str),
    RESERVED(Reserved),
    OPERATOR(OpType),
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

//token/错误的字节区间[start,end)；行列号报错时由LineIndex惰性推导，不在此存储
#[derive(Debug, PartialEq, Clone, Copy)]
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