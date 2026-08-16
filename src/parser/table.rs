//! typelua 的文法定义，`grammar!` 在编译期把它展开成 LALR(1) 的 ACTION/GOTO 静态表。
//!
//! pub const NUM_STATES:  usize ;
//! pub const NUM_SYMBOLS: usize ;
//! pub const NUM_RULES:   usize ;
//! pub const END_SYMBOL:  u32   ;

//! pub static SYMBOL_NAMES: [&str; NUM_SYMBOLS];
//! pub static IS_TERMINAL: [bool; NUM_SYMBOLS];
//! pub static RULE_LHS: [u32; NUM_RULES];
//! pub static RULE_RHS_LEN: [u32; NUM_RULES];

//! static TABLE: [i32; NUM_STATES * NUM_SYMBOLS] ;
//! const CELL_ERROR: i32;
//! const CELL_ACCEPT: i32 = i32::MAX;

//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! pub enum Action {
//!     Error,
//!     Shift(u32 /* target_state */),
//!     Reduce(u32 /* rule_index */),
//!     Accept,
//! }

//! #[inline]
//! fn cell(state: u32, symbol: u32) -> i32 ;

//! /// 终结符列：查该状态遇到这个终结符时做什么
//! pub fn action(state: u32, terminal: u32) -> Action ;

//! /// 非终结符列：归约完把左部压回栈时转向哪个状态
//! pub fn goto(state: u32, nonterminal: u32) -> Option<u32>;

//! /// 按名字找符号下标。用来把词法器的 token 种类桥接到列下标，只在初始化时调用
//! pub fn symbol_index(name: &str) -> Option<u32> ;

//! /// 该状态下所有合法的输入终结符，用于「期望以下之一」这类报错
//! pub fn expected_terminals(state: u32) -> Vec<&'static str>;


grammar::grammar! {


    //这里已经算增广文法了
    chunk
        : block
        ;
    
    block
        : stat_list
        | stat_list retstat
        | retstat
        | /* empty */
        ;
    
    stat_list
        : stat
        | stat_list stat
        ;
    
    stat
        : ';'                            /* empty stmt dropped */
        | varlist '=' explist
        | prefixexp
        | label
        | BREAK
        | GOTO NAME
        | DO block END
        | WHILE exp DO block END
        | REPEAT block UNTIL exp
        | IF exp THEN block elseif_list END
    
        | IF exp THEN block elseif_list ELSE block END
    
        | FOR NAME '=' exp ',' exp DO block END
    
        | FOR NAME '=' exp ',' exp ',' exp DO block END
    
        | FOR namelist IN explist DO block END
    
        | FUNCTION funcname funcbody
        | LOCAL FUNCTION NAME funcbody
        | LOCAL decllist
        | LOCAL decllist '=' explist
        | CLASS NAME classbody
        | CLASS NAME EXTENDS NAME classbody
    
        ;
    
    elseif_list
        : /* empty */
        | elseif_list ELSEIF exp THEN block
    
        ;
    
    retstat
        : RETURN
        | RETURN ';'
        | RETURN explist
        | RETURN explist ';'
        ;
    
    label
        : DBCOLON NAME DBCOLON
        ;
    
    funcname
        : dotted_name
        | dotted_name ':' NAME
        ;
    
    dotted_name
        : NAME
        | dotted_name '.' NAME
        ;
    
    varlist
        : var
        | varlist ',' var
        ;
    
    var
        : prefixexp
        ;
    
    namelist
        : NAME
        | namelist ',' NAME
        ;
    
    decllist
        : NAME optype
        | decllist ',' NAME optype
        ;
    
    optype
        : /* empty */
        | ':' type
        ;
    
    type
        : uniontype
        | functype
        ;
    
    uniontype
        : basictype
        | uniontype '|' basictype
        ;
    
    basictype
        : NAME
        | NIL                            /* nil type */
        | NAME '<' typeargs '>'
    
        | '(' type ')'                   /* grouping: (function()->A)|B */
        ;
    
    typeargs
        : type
        | typeargs ',' type
        ;
    
    functype
        : FUNCTION '(' ')'
        | FUNCTION '(' ')' rettype
        | FUNCTION '(' parlist ')'
        | FUNCTION '(' parlist ')' rettype
    
        ;
    
    rettype
        : ARROW retspec
        ;
    
    retspec
        : type                           /* single return */
        | '(' ')'                        /* void */
        | '(' multilist ')'
        ;

    multilist                            /* excludes the single non-vararg case:
                                            that goes through basictype grouping */
        : typelist ',' rtype             /* two or more */
        | ELLIPSIS                       /* -> (...) */
        | ELLIPSIS type                  /* -> (...any) */
        ;
    
    typelist
        : rtype
        | typelist ',' rtype
        ;
    
    rtype
        : type
        | ELLIPSIS type
        | ELLIPSIS
        ;
    
    classbody
        : '{' '}'
        | '{' classfields '}'
        ;
    
    classfields
        : classfield
        | classfields ',' classfield
        ;
    
    classfield
        : NAME ':' type
        | NAME ':' type '=' exp
        | NAME '=' exp
        | methodsig
        | methodsig block END
        | methodsig '=' exp
        ;
    
    methodsig
        : NAME '(' ')'
        | NAME '(' ')' rettype
        | NAME '(' parlist ')'
        | NAME '(' parlist ')' rettype
        ;
    
    explist
        : exp
        | explist ',' exp
        ;
    
    /* Precedence and associativity are encoded in the rules themselves:
       one nonterminal per precedence level, left recursion for left
       associativity, right recursion for right associativity. */

    exp
        : or_exp
        ;

    or_exp
        : or_exp OR and_exp
        | and_exp
        ;

    and_exp
        : and_exp AND cmp_exp
        | cmp_exp
        ;

    cmp_exp
        : cmp_exp '<' bor_exp
        | cmp_exp '>' bor_exp
        | cmp_exp LE bor_exp
        | cmp_exp GE bor_exp
        | cmp_exp EQ bor_exp
        | cmp_exp NE bor_exp
        | bor_exp
        ;

    bor_exp
        : bor_exp '|' bxor_exp
        | bxor_exp
        ;

    bxor_exp
        : bxor_exp '~' band_exp
        | band_exp
        ;

    band_exp
        : band_exp '&' sh_exp
        | sh_exp
        ;

    sh_exp
        : sh_exp SHL cat_exp
        | sh_exp SHR cat_exp
        | cat_exp
        ;

    cat_exp                              /* '..' is right associative */
        : add_exp CONCAT cat_exp
        | add_exp
        ;

    add_exp
        : add_exp '+' mul_exp
        | add_exp '-' mul_exp
        | mul_exp
        ;

    mul_exp
        : mul_exp '*' un_exp
        | mul_exp '/' un_exp
        | mul_exp IDIV un_exp
        | mul_exp '%' un_exp
        | un_exp
        ;

    un_exp                               /* unary prefix operators */
        : NOT un_exp
        | '#' un_exp
        | '-' un_exp
        | '~' un_exp
        | pow_exp
        ;

    pow_exp                              /* '^' outranks unary, yet its right
                                            operand may be unary: 2^-3 */
        : atom '^' un_exp
        | atom
        ;

    atom
        : NIL
        | TRUE
        | FALSE
        | NUMERAL
        | STRING
        | ELLIPSIS
        | functiondef
        | prefixexp
        | tableconstructor
        ;
    
    prefixexp
        : NAME
        | '(' exp ')'
        | prefixexp '[' exp ']'
        | prefixexp '.' NAME
        | prefixexp args
        | prefixexp ':' NAME args
        ;
    
    args
        : '(' ')'
        | '(' explist ')'
        | tableconstructor
        | STRING
        ;
    
    functiondef
        : FUNCTION funcbody
        ;
    
    funcbody
        : '(' ')' block END
        | '(' ')' rettype block END
        | '(' parlist ')' block END
        | '(' parlist ')' rettype block END
        ;
    
    parlist
        : params
        | params ',' vararg
        | vararg
        ;
    
    params
        : param
        | params ',' param
        ;
    
    param
        : NAME optype
        ;
    
    vararg
        : ELLIPSIS
        | ELLIPSIS type
        ;
    
    tableconstructor
        : '{' '}'
        | '{' fieldlist '}'
        ;
    
    fieldlist
        : fields
        | fields fieldsep
        ;
    
    fields
        : field
        | fields fieldsep field
        ;
    
    field
        : '[' exp ']' '=' exp
        | NAME '=' exp
        | exp
        ;
    
    fieldsep
        : ','
        | ';'
        ;
}

#[cfg(test)]
mod tests {
    //! typelua 文法的规模基线。
    //!
    //! 这些数字描述的是**这份文法**，不是 LALR 生成器本身，所以它们住在这里而不是
    //! grammar crate 里 —— 那边只保留手算得出答案的小文法。数字本身没有独立意义，
    //! 价值在于「重构生成器之后自动机不该变」：任何一个数字动了都说明分析出了偏差。
    //!
    //! 另外，这个文件能编译通过本身就是一条断言：归约-归约冲突在宏里是 panic，
    //! 所以编译成功等价于「typelua 文法是 LALR(1) 的」。

    use super::*;

    /// 扫一遍全表统计动作。ACTION 和 GOTO 共用一张表，所以要靠 IS_TERMINAL 分列
    fn tally() -> (usize, usize, usize, usize) {
        let mut shift = 0;
        let mut reduce = 0;
        let mut accept = 0;
        let mut gotos = 0;
        for state in 0..NUM_STATES as u32 {
            for symbol in 0..NUM_SYMBOLS as u32 {
                if IS_TERMINAL[symbol as usize] {
                    match action(state, symbol) {
                        Action::Error => {}
                        Action::Shift(_) => shift += 1,
                        Action::Reduce(_) => reduce += 1,
                        Action::Accept => accept += 1,
                    }
                } else if goto(state, symbol).is_some() {
                    gotos += 1;
                }
            }
        }
        (shift, reduce, accept, gotos)
    }

    #[test]
    fn table_dimensions_are_unchanged() {
        assert_eq!(NUM_STATES, 307, "LR(0) 状态数");
        assert_eq!(NUM_SYMBOLS, 118, "符号数（含 $end）");
        assert_eq!(NUM_RULES, 170, "产生式数");
        // $end 由 build_grammar 追加在最后
        assert_eq!(END_SYMBOL as usize, NUM_SYMBOLS - 1);
        assert_eq!(SYMBOL_NAMES[END_SYMBOL as usize], "$end");
        // 62 个终结符 / 56 个非终结符
        let terminals = IS_TERMINAL.iter().filter(|&&t| t).count();
        assert_eq!(terminals, 62, "终结符数");
        assert_eq!(NUM_SYMBOLS - terminals, 56, "非终结符数");
    }

    #[test]
    fn action_counts_are_unchanged() {
        let (shift, reduce, accept, gotos) = tally();
        assert_eq!(shift, 1148, "移进动作数");
        assert_eq!(gotos, 861, "goto 条目数");
        // 移进和 goto 恰好瓜分全部 LR(0) 转移
        assert_eq!(shift + gotos, 2009, "移进 + goto = 转移总数");
        assert_eq!(reduce, 4034, "归约动作数");
        assert_eq!(accept, 1, "接受动作数");
    }

    #[test]
    fn accept_is_unique_and_reachable_by_reading_a_whole_chunk() {
        // 状态 1 = goto(0, block)，[chunk -> block ·] 唯一所在的状态
        assert_eq!(action(1, END_SYMBOL), Action::Accept);
        assert_eq!(goto(0, symbol_index("block").unwrap()), Some(1));
        // 规则 0 是增广产生式 chunk -> block
        assert_eq!(SYMBOL_NAMES[RULE_LHS[0] as usize], "chunk");
        assert_eq!(RULE_RHS_LEN[0], 1);
    }

    #[test]
    fn start_state_accepts_exactly_first_of_chunk() {
        // block 可空，所以起始状态既能开始一条语句，也能直接读到输入结束
        let mut expected = expected_terminals(0);
        expected.sort();
        assert_eq!(
            expected,
            [
                "$end", "'('", "';'", "BREAK", "CLASS", "DBCOLON", "DO", "FOR", "FUNCTION",
                "GOTO", "IF", "LOCAL", "NAME", "REPEAT", "RETURN", "WHILE",
            ]
        );
    }

    #[test]
    fn three_epsilon_productions() {
        // block / elseif_list / optype 各写了一条空候选式
        let epsilon: Vec<&str> = (0..NUM_RULES)
            .filter(|&i| RULE_RHS_LEN[i] == 0)
            .map(|i| SYMBOL_NAMES[RULE_LHS[i] as usize])
            .collect();
        assert_eq!(epsilon, ["block", "elseif_list", "optype"]);
    }

    #[test]
    fn every_rule_can_be_reduced_somewhere() {
        // 规则 0 走 Accept，其余每条都必须在表里至少有一处归约，否则是死规则
        let mut reduced = vec![false; NUM_RULES];
        for state in 0..NUM_STATES as u32 {
            for symbol in 0..NUM_SYMBOLS as u32 {
                if IS_TERMINAL[symbol as usize] {
                    if let Action::Reduce(rule) = action(state, symbol) {
                        reduced[rule as usize] = true;
                    }
                }
            }
        }
        let dead: Vec<&str> = (1..NUM_RULES)
            .filter(|&i| !reduced[i])
            .map(|i| SYMBOL_NAMES[RULE_LHS[i] as usize])
            .collect();
        assert!(dead.is_empty(), "这些规则永远不会被归约：{dead:?}");
    }
}
