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
//!
//! ---
//!
//! 归约-归约冲突在宏里是 panic，所以这个文件能编译就等价于「typelua 文法是
//! LALR(1) 的」。移进-归约冲突不会 panic，而是静默贪婪移进（见 generator.rs
//! 的 set_action），实测只有 **2** 个，都是 Lua 自带的那个歧义：
//!
//!     stat : prefixexp ·      遇 '('
//!     atom : prefixexp ·      遇 '('
//!
//! 也就是 `local x = f` 换行接 `(g).y = 1`。Lua 5.1 直接报 ambiguous syntax，
//! 5.2+ 规定一律按函数调用解释，所以移进是对的。
//!
//! `as` 绑定最松（见 cast_exp）就是为了不引入第三、第四个：放在 Rust 那个位置
//! （紧于二元）会让 `x as A|B` 的 '|' 和 `x as T<U>` 的 '<' 各出一个冲突。
//! README `## grammar` 那份 bison 等价物有 4 个 —— 多的两个正是这两个，因为
//! 它的 `exp` 是扁平的；但 bison 对两者都默认移进，结果与本文法一致。
//!
//! 表里没有导出冲突数，所以这条守不住 —— 加了新语法要手动重量一遍。
//! 规模基线（337 状态 / 126 符号 / 183 产生式 / 22 标签）同理，见 helper.md。


grammar::grammar! {


    //这里已经算增广文法了
    chunk
        : block
        ;
    
    block
        : stat_list                      @Block
        | stat_list retstat              @Block
        | retstat                        @Block
        | /* empty */                    @Block
        ;
    
    stat_list
        : stat
        | stat_list stat                 @ListTail
        ;
    
    stat
        : ';'                            /* empty stmt dropped */
        | varlist '=' explist            @Assign
        | prefixexp                      @ExprStat
        | label
        | BREAK
        | GOTO NAME                      @Goto
        | DO block END                   @Do
        | WHILE exp DO block END         @While
        | REPEAT block UNTIL exp         @Repeat
        | IF exp THEN block elseif_list END @If
    
        | IF exp THEN block elseif_list ELSE block END @IfElse
    
        | FOR NAME optype '=' exp ',' exp DO block END @ForNum
    
        | FOR NAME optype '=' exp ',' exp ',' exp DO block END @ForNumStep
    
        | FOR decllist IN explist DO block END @ForIn
    
        | FUNCTION funcname funcbody     @FuncDecl
        | LOCAL FUNCTION NAME funcbody   @LocalFuncDecl
        | LOCAL decllist                 @LocalDecl
        | LOCAL decllist '=' explist     @LocalDeclInit
        | CLASS NAME generics classbody                @ClassDecl
        | CLASS NAME generics EXTENDS NAME classbody   @ClassDeclExtends
        | TYPEDEF NAME generics '=' type               @TypeDef
    
        ;
    
    elseif_list
        : /* empty */
        | elseif_list ELSEIF exp THEN block @ElseIf
    
        ;
    
    retstat
        : RETURN                         @ReturnVoid
        | RETURN ';'                     @ReturnVoid
        | RETURN explist                 @Return
        | RETURN explist ';'             @Return
        ;
    
    label
        : DBCOLON NAME DBCOLON           @Label
        ;
    
    funcname
        : dotted_name
        | dotted_name ':' NAME           @MethodName
        ;
    
    dotted_name
        : NAME
        | dotted_name '.' NAME
        ;
    
    varlist
        : var
        | varlist ',' var                @ListTail
        ;
    
    var
        : prefixexp optype               /* 标注只允许贴裸 NAME，由 checker 拦 */ @Var
        ;
    
    decllist
        : NAME optype                    @DeclFirst
        | decllist ',' NAME optype       @DeclRest
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
        | typeparams ',' NAME            @ListTail
        ;
    
    type
        : uniontype
        | functype
        ;
    
    uniontype
        : basictype
        | uniontype '|' basictype        @Union
        ;
    
    basictype
        : NAME
        | NIL                            /* nil type */
        | NAME '<' typeargs '>'          @GenericType
    
        | '(' type ')'                   /* grouping: (function()->A)|B */
        | '{' '}'                        /* 匿名 record */ @RecordEmpty
        | '{' classfieldlist '}'         /* 复用 classfieldlist：类型位只许声明形式 */ @Record
        ;
    
    typeargs
        : type
        | typeargs ',' type              @ListTail
        ;
    
    functype                             /* 类型位的参数表：名字可省，故不能复用 parlist */
        : FUNCTION '(' ')'               @FuncType
        | FUNCTION '(' ')' rettype       @FuncTypeRet
        | FUNCTION '(' argtypes ')'      @FuncTypeParams
        | FUNCTION '(' argtypes ')' rettype @FuncTypeParamsRet
        ;
    
    argtypes
        : argtypelist
        | argtypelist ',' vararg         @ParamsVararg
        | vararg
        ;
    
    argtypelist
        : argtype
        | argtypelist ',' argtype        @ListTail
        ;
    
    argtype
        : NAME ':' type                  /* 带名，纯文档 */ @ArgTypeNamed
        | type                           /* 裸类型：function(string) -> number */
        ;
    
    rettype
        : ARROW retspec                  @ReturnType
        ;
    
    retspec
        : type                           /* single return */
        | '(' ')'                        /* void */ @RetVoid
        | '(' multilist ')'              @RetMulti
        ;

    multilist                            /* 单个定长走 basictype 分组，不在这里；
                                            vararg 只许出现在末位，和 parlist 一致 */
        : typelist ',' type              /* 两个或更多定长 */ @RetFixed
        | typelist ',' vararg            /* 定长 + 末位 vararg */ @RetVararg
        | vararg                         /* 只有 vararg */
        ;
    
    typelist
        : type
        | typelist ',' type              @ListTail
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
        | explist ',' exp                @ListTail
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
        : or_exp OR and_exp              @BinOp
        | and_exp
        ;

    and_exp
        : and_exp AND cmp_exp            @BinOp
        | cmp_exp
        ;

    cmp_exp
        : cmp_exp '<' bor_exp            @BinOp
        | cmp_exp '>' bor_exp            @BinOp
        | cmp_exp LE bor_exp             @BinOp
        | cmp_exp GE bor_exp             @BinOp
        | cmp_exp EQ bor_exp             @BinOp
        | cmp_exp NE bor_exp             @BinOp
        | bor_exp
        ;

    bor_exp
        : bor_exp '|' bxor_exp           @BinOp
        | bxor_exp
        ;

    bxor_exp
        : bxor_exp '~' band_exp          @BinOp
        | band_exp
        ;

    band_exp
        : band_exp '&' sh_exp            @BinOp
        | sh_exp
        ;

    sh_exp
        : sh_exp SHL cat_exp             @BinOp
        | sh_exp SHR cat_exp             @BinOp
        | cat_exp
        ;

    cat_exp                              /* '..' is right associative */
        : add_exp CONCAT cat_exp         @BinOp
        | add_exp
        ;

    add_exp
        : add_exp '+' mul_exp            @BinOp
        | add_exp '-' mul_exp            @BinOp
        | mul_exp
        ;

    mul_exp
        : mul_exp '*' un_exp             @BinOp
        | mul_exp '/' un_exp             @BinOp
        | mul_exp IDIV un_exp            @BinOp
        | mul_exp '%' un_exp             @BinOp
        | un_exp
        ;

    un_exp                               /* unary prefix operators */
        : NOT un_exp                     @UnOp
        | '#' un_exp                     @UnOp
        | '-' un_exp                     @UnOp
        | '~' un_exp                     @UnOp
        | pow_exp
        ;

    pow_exp                              /* '^' outranks unary, yet its right
                                            operand may be unary: 2^-3 */
        : atom '^' un_exp                @BinOp
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
        | '(' exp ')'                    @Paren
        | prefixexp '[' exp ']'          @Index
        | prefixexp '.' NAME             @Dot
        | prefixexp args                 @Call
        | prefixexp ':' NAME args        @MethodCall
        | prefixexp TURBOFISH typeargs '>'    @TurboFish
        ;
    
    args
        : '(' ')'                        @ArgsEmpty
        | '(' explist ')'                @Args
        | tableconstructor
        | STRING
        ;
    
    functiondef
        : FUNCTION funcbody              @FuncExpr
        ;
    
    funcbody
        : generics '(' ')' block END     @FuncBody
        | generics '(' ')' rettype block END @FuncBodyRet
        | generics '(' parlist ')' block END @FuncBodyParams
        | generics '(' parlist ')' rettype block END @FuncBodyParamsRet
        ;
    
    parlist
        : params
        | params ',' vararg              @ParamsVararg
        | vararg
        ;
    
    params
        : param
        | params ',' param               @ListTail
        ;
    
    param
        : NAME optype                    @Param
        ;
    
    vararg
        : ELLIPSIS
        | ELLIPSIS type                  @VarargTyped
        ;
    
    tableconstructor
        : '{' '}'                        @TableEmpty
        | '{' fieldlist '}'              @Table
        ;
    
    fieldlist
        : fields
        | fields fieldsep
        ;
    
    fields
        : field
        | fields fieldsep field          @ListTail
        ;
    
    field
        : '[' exp ']' '=' exp            @FieldKV
        | NAME '=' exp                   @FieldNamed
        | exp
        ;
    
    fieldsep
        : ','
        | ';'
        ;
}
