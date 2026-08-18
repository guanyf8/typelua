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

//! /// 文法里 `@Name` 贴的产生式标签。按它 dispatch，不要按规则下标
//! pub enum Prod { … }
//! pub static RULE_PROD: [Option<Prod>; NUM_RULES];
//! pub fn prod(rule: u32) -> Option<Prod>;
//!
//! 贴标签 = 「这条产生式有语义动作」，没贴 = 「原样透传」。标签分两类：
//!
//! **擦除**：类型语言进入运行时代码的全部入口。原本只有三个
//! （`@TypeAnnotation` / `@ReturnType` / `@VarargTyped`），随着 typedef、
//! 泛型、强转、turbofish 的加入又多了四个（`@TypeDef` / `@Generics` /
//! `@Cast` / `@TurboFish`）—— 每新增一个「只有类型、却出现在代码位置」的构造，
//! 就要多一个擦除入口。类型语言内部（uniontype / functype / retspec / argtype / …）
//! 仍然一个都不用标，因为它们只能从这七处进来。
//!
//! **降级**：class 全套。它是唯一需要真代码生成的构造。


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
    
        | FOR NAME optype '=' exp ',' exp DO block END
    
        | FOR NAME optype '=' exp ',' exp ',' exp DO block END
    
        | FOR decllist IN explist DO block END
    
        | FUNCTION funcname funcbody
        | LOCAL FUNCTION NAME funcbody
        | LOCAL decllist
        | LOCAL decllist '=' explist
        | CLASS NAME generics classbody                @ClassDecl
        | CLASS NAME generics EXTENDS NAME classbody   @ClassDeclExtends
        | TYPEDEF NAME generics '=' type               @TypeDef
    
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
        : prefixexp optype               /* 标注只允许贴裸 NAME，由 checker 拦 */
        ;
    
    decllist
        : NAME optype
        | decllist ',' NAME optype
        ;
    
    optype
        : /* empty */                    /* 无标注：什么都不用做 */
        | ':' type                       @TypeAnnotation
        ;
    
    generics                             /* 泛型形参表，class / typedef / funcbody 共用 */
        : /* empty */
        | '<' typeparams '>'             @Generics
        ;
    
    typeparams
        : NAME
        | typeparams ',' NAME
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
        | '{' '}'                        /* 匿名 record */
        | '{' classfieldlist '}'         /* 复用 classfieldlist：类型位只许声明形式 */
        ;
    
    typeargs
        : type
        | typeargs ',' type
        ;
    
    functype                             /* 类型位的参数表：名字可省，故不能复用 parlist */
        : FUNCTION '(' ')'
        | FUNCTION '(' ')' rettype
        | FUNCTION '(' argtypes ')'
        | FUNCTION '(' argtypes ')' rettype
        ;
    
    argtypes
        : argtypelist
        | argtypelist ',' vararg
        | vararg
        ;
    
    argtypelist
        : argtype
        | argtypelist ',' argtype
        ;
    
    argtype
        : NAME ':' type                  /* 带名，纯文档 */
        | type                           /* 裸类型：function(string) -> number */
        ;
    
    rettype
        : ARROW retspec                  @ReturnType
        ;
    
    retspec
        : type                           /* single return */
        | '(' ')'                        /* void */
        | '(' multilist ')'
        ;

    multilist                            /* 单个定长走 basictype 分组，不在这里；
                                            vararg 只许出现在末位，和 parlist 一致 */
        : typelist ',' type              /* 两个或更多定长 */
        | typelist ',' vararg            /* 定长 + 末位 vararg */
        | vararg                         /* 只有 vararg */
        ;
    
    typelist
        : type
        | typelist ',' type
        ;
    
    classbody
        : '{' '}'                        @ClassBodyEmpty
        | '{' classfieldlist '}'         @ClassBodyFields
        ;
    
    classfieldlist                       /* 允许尾随逗号；但分隔符只能是 ','：
                                            用 fieldsep 会让 ';' 和 `stat : ';'` 打起来
                                            —— 无函数体的 methodsig 后面那个 ';' 分不清
                                            是分隔符还是方法体里的空语句 */
        : classfields
        | classfields ','
        ;
    
    classfields
        : classfield                     @ClassFieldsFirst
        | classfields ',' classfield     @ClassFieldsRest
        ;
    
    classfield
        : NAME ':' type                  @FieldDecl
        | NAME ':' type '=' exp          @FieldDeclInit
        | NAME '=' exp                   @FieldInit
        | methodsig                      @MethodDecl
        | methodsig block END            @MethodDef
        ;
    
    methodsig
        : NAME '(' ')'                        @MethodSig
        | NAME '(' ')' rettype                @MethodSigRet
        | NAME '(' parlist ')'                @MethodSigParams
        | NAME '(' parlist ')' rettype        @MethodSigParamsRet
        ;
    
    explist
        : exp
        | explist ',' exp
        ;
    
    /* Precedence and associativity are encoded in the rules themselves:
       one nonterminal per precedence level, left recursion for left
       associativity, right recursion for right associativity. */

    exp
        : cast_exp
        ;

    cast_exp                             /* 'as' 绑定最松。这样 `x as A|nil` 里的 '|' 只能
                                            归入类型，无歧义、不必加括号 —— 而这是强转最
                                            主要的用法。代价是算术中间的强转要写括号：
                                            `a + b as T` 等于 `(a + b) as T`。
                                            放在 Rust 那个位置（紧于二元、松于一元）的话，
                                            '|' 和 '<' 在强转之后既能续类型也能续表达式，
                                            阶梯里会各多出一个移进-归约冲突（实测 4 个）；
                                            放最松就回到 2 个 */
        : cast_exp AS type               @Cast
        | or_exp
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
        | prefixexp TURBOFISH typeargs '>'    @TurboFish
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
        : generics '(' ')' block END
        | generics '(' ')' rettype block END
        | generics '(' parlist ')' block END
        | generics '(' parlist ')' rettype block END
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
        | ELLIPSIS type                  @VarargTyped
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
    //!
    //! 移进-归约冲突不会 panic，而是静默贪婪移进（见 generator.rs 的 set_action）。
    //! 实测这份文法只有 **2** 个，都是 Lua 自带的那个歧义：
    //!   `stat : prefixexp ·` 和 `atom : prefixexp ·` 遇 '(' —— 即
    //!       local x = f
    //!       (g).y = 1
    //!   Lua 5.1 直接报 ambiguous syntax，5.2+ 规定一律按函数调用解释，移进正确。
    //!
    //! `as` 之所以绑定最松（见 cast_exp）就是为了不引入第三、第四个：放在 Rust 那个
    //! 位置（紧于二元）会让 `x as A|B` 的 '|' 和 `x as T<U>` 的 '<' 各出一个冲突。
    //!
    //! README `## grammar` 那份 bison 等价物有 4 个冲突 —— 多的两个正是上面这两个，
    //! 因为 bison 版的 `exp` 是扁平的（`exp : exp '|' exp`），强转之后 '|' / '<' 仍然
    //! 是合法的表达式续接。但 bison 对两者都默认移进（冲突落在 `type : uniontype`
    //! 和 `basictype : NAME` 上，这两条没有声明优先级，故优先级消解不介入），
    //! 结果与本阶梯一致 —— 两份规格是同一门语言，只是编码方式不同。
    //!
    //! 表里没有导出冲突数，所以这一条守不住 —— 加了新语法要手动重量一遍。

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

    /// 找出某个标签对应的产生式，顺便断言它恰好对应一条
    fn rule_of(p: Prod) -> usize {
        let hits: Vec<usize> = (0..NUM_RULES).filter(|&i| RULE_PROD[i] == Some(p)).collect();
        assert_eq!(hits.len(), 1, "{p:?} 应当恰好对应一条产生式，实际 {hits:?}");
        hits[0]
    }

    #[test]
    fn every_label_is_pinned_to_the_intended_production() {
        // (标签, 左部, 右部长度)。`@` 放错位置会让标签贴到相邻的候选式上，
        // 而那多半还能编译 —— 只有把左部和长度一起钉住才拦得下来
        let expected: &[(Prod, &str, u32)] = &[
            // 擦除：类型语言进入运行时代码的全部入口
            (Prod::TypeAnnotation,      "optype",      2),
            (Prod::ReturnType,          "rettype",     2),
            (Prod::VarargTyped,         "vararg",      2),
            (Prod::TypeDef,             "stat",        5),   // typedef NAME generics '=' type
            (Prod::Generics,            "generics",    3),   // '<' typeparams '>'
            (Prod::Cast,                "cast_exp",    3),   // cast_exp as type
            (Prod::TurboFish,           "prefixexp",   4),   // prefixexp ::< typeargs '>'
            // 降级：class 是唯一需要真代码生成的构造
            (Prod::ClassDecl,           "stat",        4),   // + generics
            (Prod::ClassDeclExtends,    "stat",        6),   // + generics
            (Prod::ClassBodyEmpty,      "classbody",   2),
            (Prod::ClassBodyFields,     "classbody",   3),
            (Prod::ClassFieldsFirst,    "classfields", 1),
            (Prod::ClassFieldsRest,     "classfields", 3),
            (Prod::FieldDecl,           "classfield",  3),
            (Prod::FieldDeclInit,       "classfield",  5),
            (Prod::FieldInit,           "classfield",  3),
            (Prod::MethodDecl,          "classfield",  1),
            (Prod::MethodDef,           "classfield",  3),
            (Prod::MethodSig,           "methodsig",   3),
            (Prod::MethodSigRet,        "methodsig",   4),
            (Prod::MethodSigParams,     "methodsig",   4),
            (Prod::MethodSigParamsRet,  "methodsig",   5),
        ];
        for &(prod, lhs, arity) in expected {
            let rule = rule_of(prod);
            assert_eq!(
                SYMBOL_NAMES[RULE_LHS[rule] as usize], lhs,
                "{prod:?} 贴到了 {} 上", SYMBOL_NAMES[RULE_LHS[rule] as usize]
            );
            assert_eq!(RULE_RHS_LEN[rule], arity, "{prod:?} 的右部长度不对");
        }
        // 标签总数也钉住：多贴少贴都要有人知道
        let labeled = RULE_PROD.iter().filter(|slot| slot.is_some()).count();
        assert_eq!(labeled, expected.len(), "带标签的产生式数量变了");

        // 光靠 (左部, 长度) 分不开同一左部里长度相同的候选式 —— classfield 有三条长度 3 的，
        // methodsig 有两条长度 4 的。完整判别依据是右部符号序列，但表里没发射 RULE_RHS，
        // 所以再钉一层：全部标签按规则下标排出来的顺序 = 文法里的书写顺序。
        // 这样任意两个标签互换都会被抓到
        let order: Vec<Prod> = RULE_PROD.iter().filter_map(|slot| *slot).collect();
        assert_eq!(
            order,
            [
                // stat 的最后三条候选式
                Prod::ClassDecl, Prod::ClassDeclExtends, Prod::TypeDef,
                Prod::TypeAnnotation,
                Prod::Generics,
                Prod::ReturnType,
                Prod::ClassBodyEmpty, Prod::ClassBodyFields,
                Prod::ClassFieldsFirst, Prod::ClassFieldsRest,
                Prod::FieldDecl, Prod::FieldDeclInit, Prod::FieldInit,
                Prod::MethodDecl, Prod::MethodDef,
                Prod::MethodSig, Prod::MethodSigRet,
                Prod::MethodSigParams, Prod::MethodSigParamsRet,
                // cast_exp 在表达式阶梯的最顶端（exp 之下、or_exp 之上）
                Prod::Cast,
                // turbofish 在 prefixexp 末尾
                Prod::TurboFish,
                // vararg 在 parlist 那一段，位置最靠后
                Prod::VarargTyped,
            ],
            "标签的相对顺序变了：要么 @ 贴错了候选式，要么文法重排了候选式"
        );
    }

    #[test]
    fn unlabeled_productions_mean_pass_through() {
        // 绝大多数产生式没有语义动作 —— 这不是遗漏，是「原样透传」的声明。
        // 数字钉住是为了：加了产生式却忘了考虑要不要贴标签时有人提醒
        let plain = RULE_PROD.iter().filter(|slot| slot.is_none()).count();
        assert_eq!(plain, 161);
        assert_eq!(plain + 22, NUM_RULES);
    }

    #[test]
    fn labels_do_not_disturb_the_automaton() {
        // 标签是纯元数据。规模基线不变就说明它没碰到文法本身
        // （具体数字由 table_dimensions_are_unchanged / action_counts_are_unchanged 守）
        assert_eq!(NUM_RULES, 183);
        assert_eq!(NUM_STATES, 337);
    }

    #[test]
    fn table_dimensions_are_unchanged() {
        assert_eq!(NUM_STATES, 337, "LR(0) 状态数");
        assert_eq!(NUM_SYMBOLS, 126, "符号数（含 $end）");
        assert_eq!(NUM_RULES, 183, "产生式数");
        // $end 由 build_grammar 追加在最后
        assert_eq!(END_SYMBOL as usize, NUM_SYMBOLS - 1);
        assert_eq!(SYMBOL_NAMES[END_SYMBOL as usize], "$end");
        // 65 个终结符 / 61 个非终结符
        let terminals = IS_TERMINAL.iter().filter(|&&t| t).count();
        assert_eq!(terminals, 65, "终结符数");
        assert_eq!(NUM_SYMBOLS - terminals, 61, "非终结符数");
    }

    #[test]
    fn action_counts_are_unchanged() {
        let (shift, reduce, accept, gotos) = tally();
        assert_eq!(shift, 1211, "移进动作数");
        assert_eq!(gotos, 905, "goto 条目数");
        // 移进和 goto 恰好瓜分全部 LR(0) 转移
        assert_eq!(shift + gotos, 2116, "移进 + goto = 转移总数");
        assert_eq!(reduce, 4511, "归约动作数");
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
                "GOTO", "IF", "LOCAL", "NAME", "REPEAT", "RETURN", "TYPEDEF", "WHILE",
            ]
        );
    }

    #[test]
    fn four_epsilon_productions() {
        // block / elseif_list / optype / generics 各写了一条空候选式
        let epsilon: Vec<&str> = (0..NUM_RULES)
            .filter(|&i| RULE_RHS_LEN[i] == 0)
            .map(|i| SYMBOL_NAMES[RULE_LHS[i] as usize])
            .collect();
        assert_eq!(epsilon, ["block", "elseif_list", "optype", "generics"]);
    }

    #[test]
    fn every_rule_can_be_reduced_somewhere() {
        // 规则 0 走 Accept，其余每条都必须在表里至少有一处归约，否则是死规则。
        // 这条同时守着「换掉某个非终结符之后旧的那个没删干净」：namelist 被 decllist
        // 取代、rtype 被 typelist/vararg 取代，忘了删就会在这里现形
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

    #[test]
    fn new_symbols_are_present_and_retired_ones_are_gone() {
        // 新增的三个终结符
        for t in ["TYPEDEF", "AS", "TURBOFISH"] {
            let i = symbol_index(t).unwrap_or_else(|| panic!("{t} 不在符号表里"));
            assert!(IS_TERMINAL[i as usize], "{t} 应当是终结符");
        }
        // 新增的非终结符
        for nt in ["generics", "typeparams", "classfieldlist", "argtypes", "argtypelist",
                   "argtype", "cast_exp"] {
            let i = symbol_index(nt).unwrap_or_else(|| panic!("{nt} 不在符号表里"));
            assert!(!IS_TERMINAL[i as usize], "{nt} 应当是非终结符");
        }
        // 被取代的两个必须彻底消失，不能留在符号表里
        assert_eq!(symbol_index("namelist"), None, "namelist 已被 decllist 取代");
        assert_eq!(symbol_index("rtype"), None, "rtype 已被 typelist / vararg 取代");
    }
}
