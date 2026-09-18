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
//! class 的**初始化**反而一条产生式都不占。它照 Rust 的结构体字面量来 ——
//!     local x = A{b = true, c = 1}
//! 而 `A{…}` 落在 `prefixexp args` 里 `args : tableconstructor` 那条上（Lua 自带
//! 的 `f{…}` 糖），归约成 `@Call(NAME, @Table)`，和函数调用**同形**。语法上分不开、
//! 也不该分：checker 看 prefixexp 是不是类名就能区分「构造实例」和「拿表调函数」。
//! 所以这里没有 `@ClassLit` 这种标签，emitter 也是按名字认出来再降级的。
//!
//! 连带两件事：`new` 和 `init` 都不是保留名（没有构造器，见 helper.md 12），
//! 以及子类不需要构造链 —— `B{…}` 把父类的非空字段和自己的写在同一层。
//!
//! 父类同理不占关键字：`class B : A` 用的是单字符 `':'`（第四次复用 —— 另三处是
//! 类型标注 `x:T`、方法调用 `a:f()`、funcname 的 `A:m`，四处上下文不相交）。
//! 第五次是 `typeparam` 的形参上界 `<T: number>`，它在 `'<' '>'` 内部，
//! 和父类槽的 `':'` 分属两个不同状态，`class A<T: number> : B {}` 不歧义。
//! 纯替换：状态 337、产生式 183 都不变，只是少了 `EXTENDS` 终结符（符号 126→125）。
//! `extends` 因此被释放回普通标识符，**必须同时从 type_def.rs 的 reserved! 里删掉** ——
//! adapter.rs 的 RESERVED_COL 会对它调 symbol_col()，终结符没了就 const panic。
//! 标签仍叫 `@ClassDeclExtends`：它描述的是继承关系，不是那个已消失的关键字。
//!
//! 父类槽收的是 `extendtype`（`NAME` 或 `NAME '<' typeargs '>'`），仍是单实现
//! 继承 —— 继承图是一棵树，菱形不可能出现。开泛型实参是为了让
//!     class NamedBox<T> : Box<T>      -- 实参透传
//!     class IntBox      : Box<number> -- 实参固定
//!     class Flipped<A,B>: Pair<B, A>  -- 实参重排
//! 三种形态都能写出来。不能直接拿 `basictype` 当父类槽：它的
//! `'{' classfieldlist '}'` 会和紧跟在后面的 classbody 撞成移进冲突。
//! `extendtype : NAME` 无标签单孩子，命中 parser.rs 的折叠，所以
//! `class B : A` 的 AST 跟加这个非终结符之前逐节点相同（实测 16 节点 / 深 6）。
//! `@GenericType` 直接复用类型位那个，标签数零新增。
//! ---
//!
//! 归约-归约冲突在宏里是 panic，所以这个文件能编译就等价于「typelua 文法是
//! LALR(1) 的」。移进-归约冲突不会 panic，而是静默贪婪移进（见 generator.rs
//! 的 set_action），实测只有 **2** 个，都是 Lua 自带的那个歧义：
//!
//! ```text
//! stat : prefixexp ·      遇 '('
//! atom : prefixexp ·      遇 '('
//! ```
//!
//! 也就是 `local x = f` 换行接 `(g).y = 1`。Lua 5.1 直接报 ambiguous syntax，
//! 5.2+ 规定一律按函数调用解释，所以移进是对的。
//!
//! `as` 绑定最松（见 cast_exp）就是为了不引入第三、第四个：放在 Rust 那个位置
//! （紧于二元）会让 `x as A|B` 的 '|' 和 `x as T<U>` 的 '<' 各出一个冲突。

//! 表里没有导出冲突数，所以这条守不住 —— 加了新语法要手动重量一遍。
//! 规模基线由 `cargo test parser::tests::grammar_size_report -- --nocapture` 报数，
//! 当前实测 **366 状态 / 135 符号（67 终结符）/ 199 产生式 / 73 标签**，其中 126 条
//! 产生式贴了标签、73 条透传。
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
        | LOCAL FUNCTION NAME funcbody   @FuncDecl
        | LOCAL decllist                 @LocalDecl
        | LOCAL decllist '=' explist     @LocalDeclInit
        | optpub CLASS NAME generics classbody         @ClassDecl
        | optpub CLASS NAME generics ':' extendtype classbody @ClassDeclExtends
        | optpub TYPEDEF NAME generics '=' type        @TypeDef
        | IMPORT '{' importlist '}' IN STRING          @Import
        /* 宿主声明。不能写 optype：它可空，`extern x` 得是语法错。
           「只许顶层」不在文法里，和带标注的全局赋值一样由 checker 拦；
           不给 optpub 槽，于是 `pub extern` 天然是语法错 */
        | EXTERN NAME ':' type                         @Extern

        ;

    optpub
        : /* empty */
        | PUB                            @Pub
        ;

    importlist
        : importitems
        | importitems ','
        ;

    importitems
        : importitem
        | importitems ',' importitem     @ListTail
        ;

    importitem
        : NAME                           @ImportItem
        | NAME AS NAME                   @ImportAlias
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
        : NAME                           @FuncName
        | dotted_name '.' NAME           @DottedName
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
        : typeparam
        | typeparams ',' typeparam       @ListTail
        ;

    typeparam
        : NAME                           @TypeParam
        | NAME ':' type                  @TypeParamBound
        ;

    extendtype
        : NAME                           @TypeName
        | NAME '<' typeargs '>'          @GenericType
        ;

    type
        : uniontype
        | functype
        ;

    uniontype
        : intertype
        | uniontype '|' intertype        @Union
        ;

    intertype                            /* 交类型：形状合并。比 '|' 紧，`A & B | C` = `(A&B) | C` */
        : basictype
        | intertype '&' basictype        @Intersect
        ;

    basictype
        : NAME                           @TypeName
        | NIL                            /* nil type */
        | NAME '<' typeargs '>'          @GenericType

        | '(' type ')'                   /* grouping: (function()->A)|B */ @ParenType
        | '{' '}'                        /* 匿名 record */ @RecordEmpty
        | '{' classfieldlist '}'         /* 复用 classfieldlist：类型位只许声明形式 */ @Record
        ;

    typeargs
        : type
        | typeargs ',' type              @ListTail
        ;

    functype                             /* 类型位的参数表：名字可省，故不能复用 parlist */
        : FUNCTION '(' ')'               @FuncType
        | FUNCTION '(' ')' rettype       @FuncType
        | FUNCTION '(' argtypes ')'      @FuncType
        | FUNCTION '(' argtypes ')' rettype @FuncType
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
        : '{' '}'                        @ClassBody
        | '{' classfieldlist '}'         @ClassBody
        ;

    classfieldlist                       /* 允许尾随逗号；分隔符只收 ','。
                                            原来的理由是「无函数体的 methodsig 后面那个
                                            ';' 分不清是分隔符还是方法体里的空语句」——
                                            那条候选式已经删了（见 classfield），理由随之
                                            失效。仍然只收 ',' 是为了类体和类型位的 record
                                            写法一致，没再去实测 fieldsep 那半 */
        : classfields
        | classfields ','
        ;

    classfields                          /* 单元素那条不贴标签：命中 parser.rs 的折叠，
                                            脊底直接是 classfield 本身。贴了反而多一层壳，
                                            还要 linter 的 spine() 专门剥它。
                                            左递归那条就是 `list ',' elem`，和另外九条列表
                                            逐字同形、处理也完全一样，所以共用 @ListTail */
        : classfield
        | classfields ',' classfield     @ListTail
        ;

    classfield                           /* 前三条是**属性**（普通字段，含函数变量字段）：
                                            第一条无默认值，checker 要求每一处 `Name{…}`
                                            都给出它（helper.md 12）；后两条带默认值，
                                            构造时可省。
                                            第四条是**方法**，methodsig 必须带函数体 ——
                                            「只有签名」那条候选式删掉了：方法和函数变量
                                            字段不互为糖，声明了却没人定义的方法无处收口
                                            （诊断按文件在 finish 收，定义可能在别的文件）。
                                            要一个只给签名的函数成员就写成属性：
                                            `m : function(self:A) -> T`。
                                            没有构造器，所以 init 在这里只是个普通方法名 */
        : NAME ':' type                  @FieldDecl
        | NAME ':' type '=' exp          @FieldDecl
        | NAME '=' exp                   @FieldDecl
        | methodsig block END            @MethodDef
        ;

    methodsig
        : NAME '(' ')'                        @MethodSig
        | NAME '(' ')' rettype                @MethodSig
        | NAME '(' parlist ')'                @MethodSig
        | NAME '(' parlist ')' rettype        @MethodSig
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
        : NAME                           @VarRef
        | '(' exp ')'                    @Paren
        | prefixexp '[' exp ']'          @Index
        | prefixexp '.' NAME             @Dot
        | prefixexp args                 @Call
        | prefixexp ':' NAME args        @MethodCall
        | prefixexp TURBOFISH typeargs '>'    @TurboFish
        ;

    args                                 /* tableconstructor 那条兼任 class 初始化：
                                            `A{…}` 就是它，见文件头。`A"str"` 同理是
                                            Lua 自带的糖，只是对 class 没有意义 */
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
        | generics '(' ')' rettype block END @FuncBody
        | generics '(' parlist ')' block END @FuncBody
        | generics '(' parlist ')' rettype block END @FuncBody
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
