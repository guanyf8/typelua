use crate::lexer::type_def::{OpType, Reserved, Token};
use super::table::{NOT_EXIST, SIMPLE_COL, symbol_col};

/// reserved -> index in grammar.names
pub const RESERVED_COL: [u32; Reserved::LEN] = {
    let mut t = [NOT_EXIST; Reserved::LEN];
    let mut i = 0;
    while i < Reserved::LEN {
        t[i] = symbol_col(Reserved::RESERVERD_NAMES[i]);
        i += 1;
    }
    t
};

/// complex op -> index in grammar.names
pub const OP_COL: [u32; OpType::COMPLEX_OP_LEN] = {
    let mut t = [NOT_EXIST; OpType::COMPLEX_OP_LEN];
    let mut i = 0;
    while i < OpType::COMPLEX_OP_LEN {
        t[i] = symbol_col(OpType::COMPLEX_OP_NAMES[i]);
        i += 1;
    }
    t
};


pub const COL_NAME: u32 = symbol_col("NAME");
pub const COL_NUMERAL: u32 = symbol_col("NUMERAL");
pub const COL_STRING: u32 = symbol_col("STRING");

// 拆分 SHR/GE 时要查的三列
pub const COL_GT: u32 = symbol_col("'>'");
pub const COL_SHR: u32 = symbol_col("SHR");
pub const COL_GE: u32 = symbol_col("GE");

/// token → index in grammar.names。
/// specifically, ascii 字符可以直接查表，16位字符走symbol_col(&str)
#[inline]
pub const fn col(token: &Token<'_>) -> u32 {
    match token {
        Token::NAME(_) => COL_NAME,
        Token::NUMERAL(_) => COL_NUMERAL,
        Token::STRING(_) => COL_STRING,
        Token::RESERVED(r) => RESERVED_COL[*r as usize],
        Token::OPERATOR(OpType::SIMPLE(c)) => {
            // ASCII 走数组（一次索引）；非 ASCII 的单字符终结符走
            let n = *c as usize;
            if n < 128 { SIMPLE_COL[n] } else { NOT_EXIST }
        }
        Token::OPERATOR(o) => match o.index() {
            Some(i) => OP_COL[i],
            None => NOT_EXIST,     // 不可达：SIMPLE 已拦住
        },
    }
}
