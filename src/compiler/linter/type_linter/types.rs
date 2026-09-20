//! 类型层 / 声明层 / 名字层
//!
//! 三条不变量，改动前先读：
//!
//! **① 类型是不可变值，且去重（hash-consing）。**
//! 所有类型节点只能通过本文件的构造函数进入 arena，构造时先查 intern 表。
//! 于是 `TypeId` 的 `==` 是**结构相等**而不是「同一个 id」——`Box<number>`
//! 无论构造多少次都拿到同一个 `TypeId`。这条是整个类型系统正确性的地基：
//! 一旦有人绕过构造函数直接 push，`==` 就会静默说谎。
//! 前提是规范化必须彻底：union 扁平化+排序+去重，record 字段按 NameId 排序，
//! Pack 长度 1 折叠（见各构造函数）。
//! 注意：`==` 只保证「同一个类型」，**不表示可赋值**。名义子类型（extends 链）、
//! record 的宽度子类型、函数的参数逆变/返回协变都要另写 assignable，去重帮不上忙。
//!
//! **② 类型列表是一等公民。**
//! 文法里 typeargs / argtypes / typelist / retspec 全是列表且都可能带末位 vararg，
//! 所以列表单独进 arena（`ListId`），再由 `Types::Pack` 把它包成一个类型值。
//! 属性求值因此只需要一种值（`TypeId`，4 字节 Copy），linter 不必再有
//! 「单个类型 / 类型列表」两个变体。
//! parser 会把无标签单孩子产生式折叠掉（`typeargs : type`），所以同一个语法位置
//! 可能拿到裸类型也可能拿到 Pack —— 消费点统一走 `as_list` 归一，不要各自分叉。
//!
//! **③ 声明层 ≠ 类型层。**
//! `Decl` 是可变的、有名字、有 span、要分两阶段填（先注册名字再填体，
//! 递归 typedef 与互相引用的 class 都依赖这一点）；类型值是不可变、匿名、可去重的。
//! 两者分开的直接好处：`&mut arena` 与 `&decls` 可以同时持有，subst 不必再为了
//! 绕开借用去 clone 整个 UnionInfo / FunctionInfo。
//! 字段的 span 只存在声明层（`ClassField`）：类型层的 `Field` 不带 span，
//! 否则同一个结构写在两处就会因为 span 不同而 intern 成两份。匿名 record 的
//! 逐字段诊断位置去 AST 上取。

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::lexer::type_def::Span;

// 所有 id 都是 u32 newtype：不能互相顶替，也不能和 AST 的 node_index 混用。
// 构造 `at` 与取下标 `idx` 都是模块私有，外面只能拿着 id 去问 arena。

/// 一个类型值。指向 TypeArena.types，去重后同一类型只有一个 TypeId
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(u32);
impl TypeId {
    #[inline]
    fn at(raw: usize) -> Self {
        TypeId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// 一张类型列表（定长部分 + 可选末位 vararg）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListId(u32);
impl ListId {
    #[inline]
    fn at(raw: usize) -> Self {
        ListId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// 一张 record 字段表（已按 NameId 排序）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldsId(u32);
impl FieldsId {
    #[inline]
    fn at(raw: usize) -> Self {
        FieldsId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// 一条 class / typedef 声明
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclId(u32);
impl DeclId {
    pub const ARRAY: DeclId = DeclId(0);
    /// 内建容器 `table<K,V>`
    pub const TABLE: DeclId = DeclId(1);

    /// 是不是内建容器。assignable 的「空表能进容器位」只对这两个开：
    /// `local xs:array<string> = {}` 合法，而 `class A{}` 是名义类型，
    /// 只能由 `A{…}` 造出来，那条规则没法写成通用形式
    pub const fn is_builtin_container(self) -> bool {
        matches!(self, DeclId::ARRAY | DeclId::TABLE) as bool
    }

    #[inline]
    fn at(raw: usize) -> Self {
        DeclId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// 一个泛型形参的身份。同名 T 在不同声明里是不同的 GenericId
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenericId(u32);
impl GenericId {
    #[inline]
    fn at(raw: usize) -> Self {
        GenericId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// 一个被 intern 的标识符。`Default`（NameId(0)）只是 `Context::new` 的占位：
/// current_module 在 prepare / warm_up 里总会被真正的模块名覆写后才被读
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NameId(u32);
impl NameId {
    #[inline]
    fn at(raw: usize) -> Self {
        NameId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// 一次泛型替换（形参表 → 实参表）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubstId(u32);
impl SubstId {
    #[inline]
    fn at(raw: usize) -> Self {
        SubstId(raw as u32)
    }
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

// ============================ 类型层 ============================

/// 标量与三个伪类型。**没有 void**：`-> ()` 是空 Pack，见 `TypeId::VOID`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Trival {
    /// 还没解析出来 / 这个节点不产出类型。它**不是** `-> ()`，别拿它当 void 用
    Unknown,
    /// 渐进类型的逃逸口：require 回来的值、C 层注册的对象、as 强转的落点
    Any,
    Nil,
    Boolean,
    Number,
    String,
    /// 底类型：永远求不出值。写在返回位就是「调了回不来」：prelude 里的
    /// `error` / `os.exit`，以及用户自己写的 `function fail(m:string):never`。
    /// 定值分析靠它认出 guard 写法（`if c then n=1 else error("bad") end`）
    /// 里走不出来的那一支。
    ///
    /// 它绑在**类型**上而不是变量上，所以 `os.exit()`（字段读）和
    /// `local e = error` 之后的 `e("x")` 都自动成立：类型会跟着值跑。
    ///
    /// 欠的一笔：声明了 `-> never` 的函数得真的走不出去，否则谁调它谁就被骗。
    /// 那是一条函数体末尾的可达性检查（TS 的 "cannot have a reachable
    /// end point"），靠定值分析那套区域机制在 funcbody 收口处问一句 terminated
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Types {
    Trival(Trival),
    /// 泛型形参本身
    Generic(GenericId),
    /// class / typedef 的引用 + 泛型实参（非泛型时 args 是 `ListId::EMPTY`）。
    /// **不展开**：递归 typedef 靠它终止，泛型实例化推迟到真要查字段时才做。
    /// 「是 class 还是 typedef」是声明的属性，问 `DeclTable` 即可，不占类型变体。
    /// 内建的 array<T> / table<K,V> 也走这条
    Ref {
        decl: DeclId,
        args: ListId,
    },
    /// 联合类型。variants 已扁平化、排序、去重，且长度必 >= 2
    Union(ListId),
    /// 交类型（形状合并）。和 Union 同构且共用 ListId，但必须是两个变体：
    /// 单位元和吸收元恰好互为镜像（`X|any` = any 而 `X&any` = X），合成一个
    /// 带 kind 的变体会把穷尽性检查让出去。参照 Pack —— 它和 Union 也是这么分的。
    /// 成员已排序去重、长度必 >= 2，且里面**最多一个** Record（record 之间在
    /// `intersect()` 里就已经并掉了）。Ref 不展开，所以 `{a:X} & table<K,V>` 就地保留
    Intersect(ListId),
    /// 匿名 record / class 的结构形状。字段已按 NameId 排序
    Record(FieldsId),
    /// `generics` 是这个函数自己声明的泛型形参（按声明序，每项是 `Types::Generic`，
    /// 非泛型时 `ListId::EMPTY`）。形参身份存在**类型**上而不是只留在声明里，是因为
    /// turbofish `map::<string, number>` 在使用点只拿得到一个函数值的类型，得从它
    /// 身上问出形参的顺序，才知道实参该往哪儿代
    Func {
        generics: ListId,
        params: ListId,
        ret: ListId,
    },
    /// 值列表：retspec / typeargs / argtypes。长度 1 且无 vararg 的 Pack 不存在
    /// （被折叠成元素本身，所以 `-> (T)` 天然等于 `-> T`）
    Pack(ListId),
}

/// 定长部分 + 可选末位 vararg。裸 `...` 等价 `...any`，由消费点归一后再存
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TypeList {
    pub fixed: Vec<TypeId>,
    pub vararg: Option<TypeId>,
}

impl TypeList {
    pub fn is_empty(&self) -> bool {
        self.fixed.is_empty() && self.vararg.is_none()
    }
    /// 定长个数。带 vararg 时它只是下界
    pub fn len(&self) -> usize {
        self.fixed.len()
    }
}

/// 类型层的字段：无 span（否则同形状的 record 会 intern 成两份）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Field {
    pub name: NameId,
    pub ty: TypeId,
    /// 有默认值。没有默认值的字段要求**每一处** `A{…}` 都给出它
    pub default: bool,
}

impl TypeId {
    // 预 intern 的固定 id，与 TypeArena::new 里的注册顺序一一对应
    pub const UNKNOWN: TypeId = TypeId(0);
    pub const ANY: TypeId = TypeId(1);
    pub const NIL: TypeId = TypeId(2);
    pub const BOOLEAN: TypeId = TypeId(3);
    pub const NUMBER: TypeId = TypeId(4);
    pub const STRING: TypeId = TypeId(5);
    /// 底类型。`basictype` 里没有它的关键字 —— `never` 是普通 NAME，
    /// 靠 `builtin_types` 落地，所以用户自己定义同名类型能遮蔽它
    pub const NEVER: TypeId = TypeId(6);
    /// `-> ()`：零返回值，也就是空 Pack。
    /// README 里「`()` 永远不是 type」说的是**语法层**（不进 basictype，否则
    /// `-> ()` 歧义）；语义层的空值列表只能由 retspec / argtypes 产出，不歧义
    pub const VOID: TypeId = TypeId(7);

    pub const fn of_trival(p: Trival) -> TypeId {
        match p {
            Trival::Unknown => TypeId::UNKNOWN,
            Trival::Any => TypeId::ANY,
            Trival::Nil => TypeId::NIL,
            Trival::Boolean => TypeId::BOOLEAN,
            Trival::Number => TypeId::NUMBER,
            Trival::String => TypeId::STRING,
            Trival::Never => TypeId::NEVER,
        }
    }
}

impl ListId {
    /// 空列表：非泛型引用的实参、`-> ()` 的返回表、无参函数的参数表都用它
    pub const EMPTY: ListId = ListId(0);
}

impl FieldsId {
    /// 空字段表：`{}`
    pub const EMPTY: FieldsId = FieldsId(0);
}

/// 一次替换：params[i] -> args[i]。被 intern 成 SubstId，好让 subst 的
/// memo key 退化成两个 u32
#[derive(PartialEq, Eq, Hash)]
struct Subst {
    params: Vec<GenericId>,
    args: Vec<TypeId>,
}

// ============================ 名字层 ============================

/// 标识符表。去重表与正查表共享同一份 Rc，不重复存字符串
pub struct Interner {
    strings: Vec<Rc<str>>,
    map: HashMap<Rc<str>, NameId>,
}

impl Interner {
    pub fn new() -> Self {
        Interner {
            strings: Vec::new(),
            map: HashMap::new(),
        }
    }

    pub fn intern(&mut self, s: &str) -> NameId {
        // Rc<str>: Borrow<str>，所以命中时不分配
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let rc: Rc<str> = Rc::from(s);
        let id = NameId::at(self.strings.len());
        self.strings.push(Rc::clone(&rc));
        self.map.insert(rc, id);
        id
    }

    pub fn resolve(&self, name: NameId) -> &str {
        &self.strings[name.idx()]
    }
}

// ============================ 声明层 ============================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Class,
    Typedef,
}

pub struct ClassInfo {
    /// 声明顺序保留（诊断按源码顺序报），结构比较时再交给 `TypeArena::record` 排序
    pub fields: Vec<ClassField>,
    /// 父类。存的是**类型**而不是 DeclId —— `class Flipped<A,B> : Pair<B,A>`
    /// 的实参必须留住，否则透传 / 固定 / 重排三种继承形态在类型层不可区分
    pub extends: Option<TypeId>,
}

pub struct TypedefInfo {
    /// 别名目标。递归 typedef（`typedef Node = {next:Node|nil}`）合法，
    /// 因为 body 里对自己的引用是 `Ref`，不展开
    pub target: TypeId,
}

pub enum DeclBody {
    Class(ClassInfo),
    Typedef(TypedefInfo),
}

/// 声明层的字段：类型层那份 + 源码位置 + 「是不是方法」
#[derive(Debug, Clone, Copy)]
pub struct ClassField {
    pub field: Field,
    pub span: Span,
    /// methodsig 形式（`m(self) -> T … end`）声明的**方法**。`m : function(…)`
    /// 写出来的是普通函数变量字段，这一位是 false。两者语义**不互为糖**：
    /// 方法不参与 `A{…}` 构造，也只有方法能被类体外的 `function A:m` /
    /// `function A.m` 重定义；函数变量字段反过来 —— 没默认值就得在每一处构造里给，
    /// 而且只能整体赋值，不能用 `function` 语句去定义。
    ///
    /// 不放进 `Field`：那是类型层、要参与 `intern_fields` 的哈希去重，多带这一位
    /// 会让形状相同的 record 裂成两个 TypeId
    pub method: bool,
}

pub struct Decl {
    name: NameId,
    span: Span,
    /// pub 修饰。`optpub` 先归约，所以它在 Decl 建好之后才填
    pub exported: bool,
    /// 泛型形参**只存身份**，名字/上界/位置都在 DeclTable.generics 里。
    /// 顺带让 `intern_subst(&decl.generics, &args)` 不必先 map 一遍
    pub generics: Vec<GenericId>,
    pub body: DeclBody,
}

impl Decl {
    pub fn name(&self) -> NameId {
        self.name
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn kind(&self) -> DeclKind {
        match self.body {
            DeclBody::Class(_) => DeclKind::Class,
            DeclBody::Typedef(_) => DeclKind::Typedef,
        }
    }

    pub fn as_class(&self) -> Option<&ClassInfo> {
        match &self.body {
            DeclBody::Class(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_class_mut(&mut self) -> Option<&mut ClassInfo> {
        match &mut self.body {
            DeclBody::Class(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_typedef(&self) -> Option<&TypedefInfo> {
        match &self.body {
            DeclBody::Typedef(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_typedef_mut(&mut self) -> Option<&mut TypedefInfo> {
        match &mut self.body {
            DeclBody::Typedef(t) => Some(t),
            _ => None,
        }
    }
}

/// 泛型形参的元信息。span 是形参写下的位置，诊断「Box 的 T」用
pub struct GenericInfo {
    name: NameId,
    span: Span,
    /// 形参上界 `<T : Cmp>`。约束位是单个 type（union 算一个），所以不是 Vec
    pub constraint: Option<TypeId>,
}

impl GenericInfo {
    pub fn name(&self) -> NameId {
        self.name
    }
    pub fn span(&self) -> Span {
        self.span
    }
}

/// 声明表。DeclId 全局唯一（不是文件局部下标），所以跨文件引用不必再带 FileId
pub struct DeclTable {
    decls: Vec<Decl>,
    generics: Vec<GenericInfo>,
}

impl DeclTable {
    pub fn new() -> Self {
        DeclTable {
            decls: Vec::new(),
            generics: Vec::new(),
        }
    }

    /// 两阶段注册第一步：先占位拿到 DeclId（此时体是空的），
    /// 好让类体里对自己的引用、以及互相引用的两个 class 都能解析
    pub fn declare_class(&mut self, name: NameId, span: Span) -> DeclId {
        self.push(
            name,
            span,
            DeclBody::Class(ClassInfo {
                fields: Vec::new(),
                extends: None,
            }),
        )
    }

    pub fn declare_typedef(&mut self, name: NameId, span: Span) -> DeclId {
        self.push(
            name,
            span,
            DeclBody::Typedef(TypedefInfo {
                target: TypeId::UNKNOWN,
            }),
        )
    }

    fn push(&mut self, name: NameId, span: Span, body: DeclBody) -> DeclId {
        let id = DeclId::at(self.decls.len());
        self.decls.push(Decl {
            name,
            span,
            exported: false,
            generics: Vec::new(),
            body,
        });
        id
    }

    pub fn get(&self, id: DeclId) -> &Decl {
        &self.decls[id.idx()]
    }
    pub fn get_mut(&mut self, id: DeclId) -> &mut Decl {
        &mut self.decls[id.idx()]
    }

    /// 每个形参声明处都要 fresh 一个：同名 T 在不同声明里必须是不同身份
    pub fn fresh_generic(&mut self, name: NameId, span: Span) -> GenericId {
        let id = GenericId::at(self.generics.len());
        self.generics.push(GenericInfo {
            name,
            span,
            constraint: None,
        });
        id
    }

    pub fn generic(&self, id: GenericId) -> &GenericInfo {
        &self.generics[id.idx()]
    }
    pub fn generic_mut(&mut self, id: GenericId) -> &mut GenericInfo {
        &mut self.generics[id.idx()]
    }
}

/// 类型名字空间里的一项。class / typedef / 泛型形参 / 内建标量共用一张表，
/// 所以 `basictype : NAME` 只有一条查表路径 —— 这是折叠成 Token 叶子之后
/// 唯一能统一处理裸名字的办法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRef {
    Prim(Trival),
    Decl(DeclId),
    Generic(GenericId),
}

// ============================ 类型 arena ============================

pub struct TypeArena {
    types: Vec<Types>,
    /// 与 types 平行：子树里是否出现过泛型形参，也就是这个类型还是个待实例化的模板。subst 靠它剪枝，非泛型代码路径零开销
    type_generic: Vec<bool>,
    intern: HashMap<Types, TypeId>,

    lists: Vec<Rc<TypeList>>,
    list_generic: Vec<bool>,
    list_intern: HashMap<Rc<TypeList>, ListId>,

    fields: Vec<Rc<[Field]>>,
    fields_generic: Vec<bool>,
    fields_intern: HashMap<Rc<[Field]>, FieldsId>,

    substs: Vec<Rc<Subst>>,
    subst_intern: HashMap<Rc<Subst>, SubstId>,
    subst_memo: HashMap<(TypeId, SubstId), TypeId>,
}

impl TypeArena {
    pub fn new() -> Self {
        let mut a = TypeArena {
            types: Vec::new(),
            type_generic: Vec::new(),
            intern: HashMap::new(),
            lists: Vec::new(),
            list_generic: Vec::new(),
            list_intern: HashMap::new(),
            fields: Vec::new(),
            fields_generic: Vec::new(),
            fields_intern: HashMap::new(),
            substs: Vec::new(),
            subst_intern: HashMap::new(),
            subst_memo: HashMap::new(),
        };

        // 顺序即 id，和 TypeId::* / ListId::EMPTY / FieldsId::EMPTY 的常量绑死
        let empty_list = a.intern_list(Vec::new(), None);
        let empty_fields = a.intern_fields(Vec::new());
        for p in [
            Trival::Unknown,
            Trival::Any,
            Trival::Nil,
            Trival::Boolean,
            Trival::Number,
            Trival::String,
            Trival::Never,
        ] {
            a.intern(Types::Trival(p));
        }
        let void = a.intern(Types::Pack(ListId::EMPTY));

        debug_assert_eq!(empty_list, ListId::EMPTY);
        debug_assert_eq!(empty_fields, FieldsId::EMPTY);
        debug_assert_eq!(a.intern(Types::Trival(Trival::Unknown)), TypeId::UNKNOWN);
        debug_assert_eq!(a.intern(Types::Trival(Trival::String)), TypeId::STRING);
        debug_assert_eq!(a.intern(Types::Trival(Trival::Never)), TypeId::NEVER);
        debug_assert_eq!(void, TypeId::VOID);
        a
    }

    // ---- 正查 ----

    pub fn get_type(&self, ty: TypeId) -> Types {
        self.types[ty.idx()]
    }
    pub fn list(&self, id: ListId) -> &TypeList {
        &self.lists[id.idx()]
    }
    pub fn fields(&self, id: FieldsId) -> &[Field] {
        &self.fields[id.idx()]
    }
    /// 子树里有泛型形参没被替换掉
    pub fn contains_generic(&self, ty: TypeId) -> bool {
        self.type_generic[ty.idx()]
    }

    // ---- 构造：一律走这里，规范化 + 去重 ----

    fn intern(&mut self, t: Types) -> TypeId {
        if let Some(&hit) = self.intern.get(&t) {
            return hit;
        }
        let has_generic = match t {
            Types::Trival(_) => false,
            Types::Generic(_) => true,
            Types::Ref { args, .. } => self.list_generic[args.idx()],
            Types::Union(l) | Types::Intersect(l) | Types::Pack(l) => self.list_generic[l.idx()],
            Types::Record(f) => self.fields_generic[f.idx()],
            // generics 那格也得算进来：`function<T>() -> number` 的形参一次没用上，
            // 可 subst 开头那道 contains_generic 快路要是把它判成「不含泛型」，
            // 整条替换就被跳过、generics 永远摘不空
            Types::Func {
                generics,
                params,
                ret,
            } => {
                self.list_generic[generics.idx()]
                    || self.list_generic[params.idx()]
                    || self.list_generic[ret.idx()]
            }
        };
        let id = TypeId::at(self.types.len());
        self.types.push(t);
        self.type_generic.push(has_generic);
        self.intern.insert(t, id);
        id
    }

    pub fn intern_list(&mut self, fixed: Vec<TypeId>, vararg: Option<TypeId>) -> ListId {
        let key: Rc<TypeList> = Rc::new(TypeList { fixed, vararg });
        if let Some(&hit) = self.list_intern.get(&key) {
            return hit;
        }
        let has_generic = key.fixed.iter().any(|&t| self.contains_generic(t))
            || key.vararg.is_some_and(|t| self.contains_generic(t));
        let id = ListId::at(self.lists.len());
        self.lists.push(Rc::clone(&key));
        self.list_generic.push(has_generic);
        self.list_intern.insert(key, id);
        id
    }

    /// 字段表按 NameId 排序后去重。重名字段是用户错误，由 linter 报诊断；
    /// 这里的 dedup 只为守住「同形状 → 同 id」这条不变量
    fn intern_fields(&mut self, mut fields: Vec<Field>) -> FieldsId {
        fields.sort_by_key(|f| f.name);
        fields.dedup_by_key(|f| f.name);
        let key: Rc<[Field]> = Rc::from(fields);
        if let Some(&hit) = self.fields_intern.get(&key) {
            return hit;
        }
        let has_generic = key.iter().any(|f| self.contains_generic(f.ty));
        let id = FieldsId::at(self.fields.len());
        self.fields.push(Rc::clone(&key));
        self.fields_generic.push(has_generic);
        self.fields_intern.insert(key, id);
        id
    }

    pub fn generic(&mut self, g: GenericId) -> TypeId {
        self.intern(Types::Generic(g))
    }

    pub fn reference(&mut self, decl: DeclId, args: ListId) -> TypeId {
        self.intern(Types::Ref { decl, args })
    }

    /// 扁平化 + 排序 + 去重。`A|B` 与 `B|A`、`(A|B)|C` 与 `A|(B|C)` 因此相等。
    /// 排序键是 TypeId 的 id（intern 顺序），只要求是个稳定的全序 ——
    /// 代价是打印出来的 variant 次序不是书写次序
    pub fn union(&mut self, variants: Vec<TypeId>) -> TypeId {
        let mut flat: Vec<TypeId> = Vec::with_capacity(variants.len());
        for v in variants {
            match self.get_type(v) {
                Types::Union(l) => flat.extend_from_slice(&self.list(l).fixed.clone()),
                _ => flat.push(v),
            }
        }
        flat.sort_unstable();
        flat.dedup();
        match flat.len() {
            0 => TypeId::UNKNOWN,
            1 => flat[0],
            _ => {
                let l = self.intern_list(flat, None);
                self.intern(Types::Union(l))
            }
        }
    }

    /// 交类型：扁平化 + 丢单位元 + 急合并 record + 排序去重。和 `union` 同构，
    /// 但三条规则相反或独有，别照拄：
    ///   - `any` / `unknown` 是交的单位元，直接丢（TS 相反：`X & any` = any）。
    ///     理由是 ANY 在这门语言里是逃逸口，交里保留 X 的信息比抹掉有用。
    ///     全丢空了才回退：有 unknown 成员就还 UNKNOWN（保住「上游没解析出来」
    ///     这个信号），否则还 ANY —— 交的顶。注意和 `union` 的 0 个成员 -> UNKNOWN 不同
    ///   - 两个 record 当场并成一个（record 没有名义身份要保），重名字段递归取交
    ///   - `Ref` 绩不展开（递归终结点），所以 `{a:X} & table<K,V>` 就地保留两个成员
    ///
    /// 矛盾交（`number & string`）不在这里报错，类型层只干归一化 —— 判定见
    /// `intersection_conflict`，诊断归 linter，和 `as_array` / `as_map` 一个分工
    pub fn intersect(&mut self, parts: Vec<TypeId>) -> TypeId {
        let mut flat: Vec<TypeId> = Vec::with_capacity(parts.len());
        let mut saw_unknown = false;
        for p in parts {
            match self.get_type(p) {
                Types::Intersect(l) => flat.extend_from_slice(&self.list(l).fixed.clone()),
                Types::Trival(Trival::Any) => {}
                Types::Trival(Trival::Unknown) => saw_unknown = true,
                _ => flat.push(p),
            }
        }
        // record 全并成一个，其余成员（Ref / Generic / Func / 标量 / Union）原样留着
        let mut merged: Option<Vec<Field>> = None;
        let mut rest: Vec<TypeId> = Vec::with_capacity(flat.len());
        for t in flat {
            match self.get_type(t) {
                Types::Record(f) => {
                    let add = self.fields(f).to_vec();
                    merged = Some(match merged {
                        None => add,
                        Some(acc) => self.merge_fields(acc, add),
                    });
                }
                _ => rest.push(t),
            }
        }
        if let Some(fields) = merged {
            let r = self.record(fields);
            rest.push(r);
        }
        rest.sort_unstable();
        rest.dedup();
        match rest.len() {
            0 if saw_unknown => TypeId::UNKNOWN,
            0 => TypeId::ANY,
            1 => rest[0],
            _ => {
                let l = self.intern_list(rest, None);
                self.intern(Types::Intersect(l))
            }
        }
    }

    /// 两张字段表按 name 归并。**不能**拼起来丢给 `record()` —— `intern_fields` 的
    /// `dedup_by_key` 重名只留第一个，`{x:number} & {x:string}` 会静默变成 `{x:number}`，
    /// 是个错答案而不是报错。重名时字段类型递归取交，`default` 取 `&&`：
    /// 只有两边都有默认值才算可省 —— 宁可多要一次初始化，也别漏掉检查。
    /// 递归会终止：字段类型严格更小，而 record 只能通过不展开的 Ref 引用自己
    fn merge_fields(&mut self, acc: Vec<Field>, add: Vec<Field>) -> Vec<Field> {
        let mut out = acc;
        for f in add {
            match out.iter().position(|o| o.name == f.name) {
                Some(i) => {
                    out[i].ty = self.intersect(vec![out[i].ty, f.ty]);
                    out[i].default &= f.default;
                }
                None => out.push(f),
            }
        }
        out
    }

    /// 交里有没有明摆着不可能共存的成员，返回第一对。判据故意窄：只认「两个不同
    /// 的标量」—— `any` 已经在 `intersect` 里丢掉，剩下 nil/boolean/number/string 两两
    /// 互斥没争议（成员已去重，两个标量必不相等）。`number & {x:T}` 这类算不算得等
    /// assignable 定下子类型关系再说 —— Lua 的 string 还有元表，未必是错的。
    /// 不引入 NEVER：底类型会污染 assignable 的每条分支。
    /// 得往下走：急合并会把矛盾塞进字段里（`{x:number} & {x:string}` 已经变成
    /// `{x: number & string}`）。Ref 不展开、Generic 是叶子，所以递归自然终止，同 write_type
    pub fn intersection_conflict(&self, ty: TypeId) -> Option<(TypeId, TypeId)> {
        match self.get_type(ty) {
            Types::Intersect(l) => {
                let members = &self.list(l).fixed;
                let mut scalars = members
                    .iter()
                    .copied()
                    .filter(|&m| matches!(self.get_type(m), Types::Trival(_)));
                if let (Some(a), Some(b)) = (scalars.next(), scalars.next()) {
                    return Some((a, b));
                }
                members.iter().find_map(|&m| self.intersection_conflict(m))
            }
            Types::Record(f) => self
                .fields(f)
                .iter()
                .find_map(|fld| self.intersection_conflict(fld.ty)),
            Types::Union(l) | Types::Pack(l) => self.list_conflict(l),
            // generics 只是形参的身份表，不谈居民，不进这一问
            Types::Func { params, ret, .. } => self
                .list_conflict(params)
                .or_else(|| self.list_conflict(ret)),
            // Ref 的实参也要看：`array<number & string>` 同样无居民
            Types::Ref { args, .. } => self.list_conflict(args),
            Types::Trival(_) | Types::Generic(_) => None,
        }
    }

    fn list_conflict(&self, l: ListId) -> Option<(TypeId, TypeId)> {
        let list = self.list(l);
        list.fixed
            .iter()
            .find_map(|&m| self.intersection_conflict(m))
            .or_else(|| list.vararg.and_then(|v| self.intersection_conflict(v)))
    }

    pub fn record(&mut self, fields: Vec<Field>) -> TypeId {
        let f = self.intern_fields(fields);
        self.intern(Types::Record(f))
    }

    pub fn func(&mut self, generics: ListId, params: ListId, ret: ListId) -> TypeId {
        self.intern(Types::Func {
            generics,
            params,
            ret,
        })
    }

    /// 长度 1 且无 vararg 时折叠成元素本身 —— `-> (T)` 等于 `-> T` 这条语义
    /// 就落在这里，同时也让 parser 的单孩子折叠不再需要消费点分叉
    pub fn pack_of(&mut self, l: ListId) -> TypeId {
        let list = self.list(l);
        if list.vararg.is_none() && list.fixed.len() == 1 {
            return list.fixed[0];
        }
        self.intern(Types::Pack(l))
    }

    pub fn pack(&mut self, fixed: Vec<TypeId>, vararg: Option<TypeId>) -> TypeId {
        let l = self.intern_list(fixed, vararg);
        self.pack_of(l)
    }

    /// 归一：把「可能是 Pack、也可能被折叠成裸类型」的值统一看成列表。
    /// typeargs / argtypes / retspec 的消费点都该走它
    pub fn as_list(&mut self, ty: TypeId) -> ListId {
        match self.get_type(ty) {
            Types::Pack(l) => l,
            _ => self.intern_list(vec![ty], None),
        }
    }

    // ---- 内建容器的拆解 ----
    // 容器就是 `Ref{decl: ARRAY/TABLE, args}`，这两个只负责把实参拆出来。
    // 「下标得是 number」「键能不能赋给 K」这类诊断是 linter 的事，不在类型层

    /// `array<T>` -> T。arity 已由 resolve_generic_type 把守，这里拿不到就算不是数组。
    /// 交类型里扫成员：`{a:X} & array<T>` 的 `t[i]` 靠这条拿到 T
    pub fn as_array(&self, ty: TypeId) -> Option<TypeId> {
        match self.get_type(ty) {
            Types::Ref { decl, args } if decl == DeclId::ARRAY => {
                self.list(args).fixed.first().copied()
            }
            Types::Intersect(l) => self.list(l).fixed.iter().find_map(|&m| self.as_array(m)),
            _ => None,
        }
    }

    /// `table<K,V>` -> (K, V)；交类型同样扫成员（`{a:X} & table<K,V>` 就是动机 A）
    pub fn as_map(&self, ty: TypeId) -> Option<(TypeId, TypeId)> {
        match self.get_type(ty) {
            Types::Ref { decl, args } if decl == DeclId::TABLE => {
                let fixed = &self.list(args).fixed;
                Some((*fixed.first()?, *fixed.get(1)?))
            }
            Types::Intersect(l) => self.list(l).fixed.iter().find_map(|&m| self.as_map(m)),
            _ => None,
        }
    }

    // ---- 泛型替换 ----

    /// params[i] -> args[i]。长度不等时多出来的形参保持原样（arity 错由 linter 报）
    pub fn intern_subst(&mut self, params: &[GenericId], args: &[TypeId]) -> SubstId {
        let key = Rc::new(Subst {
            params: params.to_vec(),
            args: args.to_vec(),
        });
        if let Some(&hit) = self.subst_intern.get(&key) {
            return hit;
        }
        let id = SubstId::at(self.substs.len());
        self.substs.push(Rc::clone(&key));
        self.subst_intern.insert(key, id);
        id
    }

    /// 把 ty 里的泛型形参换成实参。纯函数 + 记忆化：
    /// 同一个 (类型, 替换) 只算一次，所以泛型类反复实例化不会让 arena 膨胀。
    /// 不递归进 `Ref` 指向的**定义体** —— 只换它的实参，定义体在查字段时才展开，
    /// 这是递归 typedef 不炸栈的原因
    pub fn subst(&mut self, ty: TypeId, s: SubstId) -> TypeId {
        if !self.contains_generic(ty) {
            return ty;
        }
        if let Some(&hit) = self.subst_memo.get(&(ty, s)) {
            return hit;
        }
        let out = match self.get_type(ty) {
            Types::Trival(_) => ty,
            Types::Generic(g) => {
                let sub = &self.substs[s.idx()];
                match sub.params.iter().position(|&p| p == g) {
                    Some(i) => sub.args.get(i).copied().unwrap_or(ty),
                    None => ty,
                }
            }
            Types::Ref { decl, args } => {
                let args = self.subst_list(args, s);
                self.reference(decl, args)
            }
            Types::Union(l) => {
                // 替换后可能出现重复或嵌套 union（实参本身是 union），所以要重新规范化
                let variants = self.list(l).fixed.clone();
                let mut out = Vec::with_capacity(variants.len());
                for v in variants {
                    out.push(self.subst(v, s));
                }
                self.union(out)
            }
            Types::Intersect(l) => {
                // 代换后形状会塌缩（`T & {x:number}` 里 T = `{y:string}` 就并成一个
                // record），所以必须回 `intersect()` 重新范式化，不能裸 intern
                let parts = self.list(l).fixed.clone();
                let mut out = Vec::with_capacity(parts.len());
                for p in parts {
                    out.push(self.subst(p, s));
                }
                self.intersect(out)
            }
            Types::Record(f) => {
                let mut fields = self.fields(f).to_vec();
                for fld in fields.iter_mut() {
                    fld.ty = self.subst(fld.ty, s);
                }
                self.record(fields)
            }
            Types::Func {
                generics,
                params,
                ret,
            } => {
                // 被这次替换代掉的形参不再是形参：generics 逐个试代，只留下没换走的
                // （部分代入会剩几个）。turbofish 全代完就摘空，于是
                // `f::<number>::<string>` 第二次自然会被认成「不是泛型函数」
                let declared = self.list(generics).fixed.clone();
                let kept: Vec<TypeId> = declared
                    .into_iter()
                    .filter(|&g| self.subst(g, s) == g)
                    .collect();
                let generics = self.intern_list(kept, None);
                let params = self.subst_list(params, s);
                let ret = self.subst_list(ret, s);
                self.func(generics, params, ret)
            }
            Types::Pack(l) => {
                let l = self.subst_list(l, s);
                self.pack_of(l)
            }
        };
        self.subst_memo.insert((ty, s), out);
        out
    }

    pub fn subst_list(&mut self, id: ListId, s: SubstId) -> ListId {
        if !self.list_generic[id.idx()] {
            return id;
        }
        let TypeList { fixed, vararg } = self.list(id).clone();
        let mut new_fixed = Vec::with_capacity(fixed.len());
        for t in fixed {
            new_fixed.push(self.subst(t, s));
        }
        let new_vararg = vararg.map(|t| self.subst(t, s));
        self.intern_list(new_fixed, new_vararg)
    }
}

// ============================ 会话 ============================

/// 变量的一格。比裸 `TypeId` 多出来的几样都是「先声明后使用」要用的：
/// `inited` 做定值分析，`decl_span` 让「后续赋值背叛声明」的诊断能指回声明点，
/// `kind` 定后续赋值该报哪种诊断、以及要不要生代码
#[derive(Debug, Clone, Copy)]
pub struct VarInfo {
    pub ty: TypeId,
    /// 已确定赋过值。声明时的初值 = 「声明处就给了值」或「ty 容得下 nil」——
    /// 老规则「无初值的 `local x:T` 要求 T 容得下 nil」由此从合法性闸门
    /// 降级成这一位的初值：容得下 nil 的类型，声明出来的那个 nil 就是它的合法值
    pub inited: bool,
    pub decl_span: Span,
    pub kind: VarScope,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VarScope {
    /// `local` / 形参 / for 变量
    Local,
    /// 顶层第一次出现的全局
    Global,
    /// `extern`：宿主注入。声明处就算已赋值（和「不核实存在性」同一个
    /// 信任模型），且零代码生成
    Extern,
    /// 顶层 `function Name funcbody` 抬升上来的名字。没执行到它的声明语句之前
    /// `inited` 是 false，于是顶层立即位置读它会报「尚未赋值」而函数体内不查 ——
    /// 抬升那条「只在延迟求值位置生效」和定值分析共用同一位
    HoistedFn,
}

/// 全工程共享的编译期状态。
/// 类型表与声明表都在这一层，不再按文件切分：
///   - intern 表共享才有最大收益（各文件里的 `array<string>` 是同一个 TypeId）
///   - DeclId 全局唯一，import 进来的类型不必重映射进本文件 arena，
///     `Global` 里存裸 id 定位不到 arena 的老问题一并消失
/// 类型层纯编译期、擦除后一个字都不剩，所以这么做和「pub 只影响可见性」不冲突
pub struct Session {
    pub names: Interner,
    pub type_arenas: TypeArena,
    pub decls: DeclTable,
    /// pub 出去的类型：(模块名, 类型名) -> 声明
    exports: HashMap<(NameId, NameId), DeclId>,
    /// 模块里**所有**顶层类型（pub 与否都算）：(模块名, 类型名) -> 声明。
    /// import 查不到时靠它分辨「模块压根没这个类型」与「有、但没 pub」：
    /// `exports` 里的都是 pub 过的，单靠它俩这两种误差拆不开
    module_types: HashMap<(NameId, NameId), DeclId>,
    /// 全局变量表。全局是 `_ENV` 的字段、整个工程同一份，所以它跟着
    /// Session 攒，不进 per-file 重置的 scope_stack（README §15：全局变量
    /// 的标注得收进工程级命名空间，否则别的文件看不到 `G_config` 的类型）。
    /// linter 查变量时把它当作作用域链的最外一环兜底，不复制进 scope_stack：
    /// 只有一份才不会不一致。
    ///
    /// 键用 String 而不是 NameId：消费者是 linter 那边 String 键的作用域表，
    /// 而且 `&self` 的查表位拿不到 `&mut names`，没法顺手 intern
    globals: HashMap<String, VarInfo>,
    /// 内建类型名：`array` / `table` 以及四个标量名。它们是 prelude 级的，
    /// 对所有文件可见，所以不能待在 per-file 的 scope_stack 里 —— linter 查
    /// 类型名时栈内全落空才问到这里，于是用户自己声明的同名类型自然遮蔽内建
    builtin_types: HashMap<String, TypeRef>,
}

impl Session {
    pub fn new() -> Self {
        let mut session = Session {
            names: Interner::new(),
            type_arenas: TypeArena::new(),
            decls: DeclTable::new(),
            exports: HashMap::new(),
            module_types: HashMap::new(),
            globals: HashMap::new(),
            builtin_types: HashMap::new(),
        };
        session.register_builtins();
        session.register_prelude();
        session
    }

    /// 注册内建类型名。`array<T>` / `table<K,V>` 是**无字段的 class**：
    /// class 是名义类型、`Ref` 不展开，所以 `array<string>` 与 `array<number>`
    /// 天然不相容；换成 typedef 就会被展开成 body，实参丢在路上。
    /// 它们没有字段（Lua 里数组不带方法，`table.insert(xs, v)` 才是常态），
    /// 索引规则靠 `TypeArena::as_array` / `as_map` 拆实参。
    ///
    /// 四个标量名在词法上是普通 NAME（不是关键字），不在这里落地就没人
    /// 认得 `local x:number`；`nil` 不在列表里 —— 它是关键字 basictype，走语法不走查表。
    ///
    /// **必须是 DeclTable 的第一个写入者**：`DeclId::ARRAY` / `TABLE` 和这里的
    /// 注册顺序绑死，末尾的 debug_assert 就是这条约束的看门人
    fn register_builtins(&mut self) {
        let array = self.declare_builtin("array", &["T"]);
        let table = self.declare_builtin("table", &["K", "V"]);
        for (name, prim) in [
            ("any", Trival::Any),
            ("boolean", Trival::Boolean),
            ("number", Trival::Number),
            ("string", Trival::String),
            // never 在这里，`unknown` 不在：前者是 prelude 和用户都要写的返回位
            // 标注（`extern error:function(any)->never`），后者是「还没算出来」这个
            // 内部状态，一旦能写就给了用户一个关闭检查的后门
            ("never", Trival::Never),
        ] {
            self.builtin_types
                .insert(name.to_string(), TypeRef::Prim(prim));
        }
        debug_assert_eq!(array, DeclId::ARRAY);
        debug_assert_eq!(table, DeclId::TABLE);
    }

    /// 一个内建容器：声明 + 泛型形参 + 落进内建名字表。
    /// span 取默认值 —— 代码里注册的东西没有源文件位置。
    /// 形参必须真的 fresh 出来：resolve_generic_type 的 arity 检查读的是
    /// `decl.generics.len()`，不填就会把 `array<string>` 当成「要 0 个实参」
    fn declare_builtin(&mut self, name: &str, generics: &[&str]) -> DeclId {
        let name_id = self.names.intern(name);
        let decl = self.decls.declare_class(name_id, Span::default());
        let params: Vec<GenericId> = generics
            .iter()
            .map(|g| {
                let g = self.names.intern(g);
                self.decls.fresh_generic(g, Span::default())
            })
            .collect();
        self.decls.get_mut(decl).generics = params;
        self.builtin_types
            .insert(name.to_string(), TypeRef::Decl(decl));
        decl
    }

    /// 标准库的全局。README「声明点」：这一份形态上等价于一串 `extern`，
    /// 没它的话「先声明后使用」会让每个文件都报一屏未声明。
    ///
    /// 口径跟 `extern` 一模一样：`VarScope::Extern`、声明处就算已赋值、
    /// 只声明存在性而不核实存在性。又因为 `declare_global` 遇重名返回 false
    /// 而不覆写，用户自己的 `extern print:…` 会报「已经声明过了」—— 这是对的：
    /// 标准库的名字就是已经占住的。
    ///
    /// 类型尽量宽：这些签名是给用户代码兼容的，宁可漏报不误报。
    /// 真正只能写死的是两处：`error` / `os.exit` 的 `-> never`（可达性
    /// 分析的 guard 写法就靠它），以及 `os.getenv` 的 `-> string|nil`（nil 收窄）
    fn register_prelude(&mut self) {
        let (any, s, n, b, nil) = (
            TypeId::ANY,
            TypeId::STRING,
            TypeId::NUMBER,
            TypeId::BOOLEAN,
            TypeId::NIL,
        );
        let str_or_nil = self.type_arenas.union(vec![s, nil]);
        let num_or_nil = self.type_arenas.union(vec![n, nil]);
        let void: Vec<TypeId> = vec![];

        // ---- 基础库函数 ----
        for (name, params, vararg, ret) in [
            ("print", void.clone(), Some(any), void.clone()),
            ("require", vec![s], None, vec![any]),
            ("tostring", vec![any], None, vec![s]),
            // 第二个实参是进制，可省；推不出数时返 nil
            ("tonumber", vec![any], Some(any), vec![num_or_nil]),
            ("type", vec![any], None, vec![s]),
            // 条件为真时原样返回第一个实参，所以**不能**声明成 `-> never`
            ("assert", vec![any], Some(any), vec![any]),
            // pcall 第一位是成败位，后面跟着被调者的返回值
            ("pcall", vec![any], Some(any), vec![b]),
            ("xpcall", vec![any, any], Some(any), vec![b]),
            ("select", vec![any], Some(any), vec![any]),
            ("rawget", vec![any, any], None, vec![any]),
            ("rawset", vec![any, any, any], None, vec![any]),
            ("rawequal", vec![any, any], None, vec![b]),
            ("rawlen", vec![any], None, vec![n]),
            ("setmetatable", vec![any, any], None, vec![any]),
            ("getmetatable", vec![any], None, vec![any]),
            // 迭代器三件套。for-in 的控制变量现在不从返回类型里抽，
            // 所以这里只需要名字存在、实参个数对得上
            ("ipairs", vec![any], None, vec![any, any, n]),
            ("pairs", vec![any], None, vec![any, any, nil]),
            ("next", vec![any], Some(any), vec![any, any]),
            ("unpack", vec![any], Some(any), vec![any]),
            ("collectgarbage", void.clone(), Some(any), vec![any]),
            ("load", vec![any], Some(any), vec![any]),
            ("dofile", void.clone(), Some(any), vec![any]),
        ] {
            let ty = self.prelude_func(params, vararg, ret);
            self.prelude_global(name, ty);
        }

        // `error` 回不来：`if c then n=1 else error("bad") end` 能过就靠这一行
        let never = self.prelude_func(vec![any], Some(any), vec![TypeId::NEVER]);
        self.prelude_global("error", never);

        // ---- 库表 ----
        let string_lib = self.prelude_lib(&[
            ("len", vec![s], None, vec![n]),
            ("sub", vec![s, n], Some(n), vec![s]),
            ("upper", vec![s], None, vec![s]),
            ("lower", vec![s], None, vec![s]),
            ("rep", vec![s, n], Some(s), vec![s]),
            ("reverse", vec![s], None, vec![s]),
            ("byte", vec![s], Some(n), vec![n]),
            ("char", vec![], Some(n), vec![s]),
            ("format", vec![s], Some(any), vec![s]),
            ("find", vec![s, s], Some(any), vec![any]),
            ("match", vec![s, s], Some(any), vec![any]),
            ("gmatch", vec![s, s], None, vec![any]),
            ("gsub", vec![s, s, any], Some(any), vec![s, n]),
        ]);
        self.prelude_global("string", string_lib);

        // 库表叫 `table`，内建**类型**也叫 `table` —— 不碰：一个在 globals、
        // 一个在 builtin_types，查表的位置分得开（值位 / 类型位）
        let table_lib = self.prelude_lib(&[
            ("insert", vec![any, any], Some(any), vec![]),
            ("remove", vec![any], Some(n), vec![any]),
            ("concat", vec![any], Some(any), vec![s]),
            ("sort", vec![any], Some(any), vec![]),
            ("unpack", vec![any], Some(any), vec![any]),
            ("pack", vec![], Some(any), vec![any]),
        ]);
        self.prelude_global("table", table_lib);

        let math_lib = self.prelude_lib(&[
            ("floor", vec![n], None, vec![n]),
            ("ceil", vec![n], None, vec![n]),
            ("abs", vec![n], None, vec![n]),
            ("max", vec![n], Some(n), vec![n]),
            ("min", vec![n], Some(n), vec![n]),
            ("sqrt", vec![n], None, vec![n]),
            ("random", vec![], Some(n), vec![n]),
            ("fmod", vec![n, n], None, vec![n]),
            ("tointeger", vec![any], None, vec![num_or_nil]),
            ("type", vec![any], None, vec![str_or_nil]),
        ]);
        // 常量字段得单独拼：prelude_lib 只会造函数字段
        let math_lib = self.prelude_extend(
            math_lib,
            &[("pi", n), ("huge", n), ("maxinteger", n), ("mininteger", n)],
        );
        self.prelude_global("math", math_lib);

        let os_lib = self.prelude_lib(&[
            // getenv 可能没有：README 的 nil 收窄例子直接拿它开头
            ("getenv", vec![s], None, vec![str_or_nil]),
            ("time", vec![], Some(any), vec![n]),
            ("clock", vec![], None, vec![n]),
            ("date", vec![], Some(any), vec![s]),
            ("remove", vec![s], None, vec![any]),
            ("rename", vec![s, s], None, vec![any]),
            // exit 和 error 同理，而且还钉住了「判据在类型上不在写法上」：
            // 它是个字段读，可达性分析照样认得出来
            ("exit", vec![], Some(any), vec![TypeId::NEVER]),
        ]);
        self.prelude_global("os", os_lib);

        let io_lib = self.prelude_lib(&[
            ("write", vec![], Some(any), vec![any]),
            ("read", vec![], Some(any), vec![any]),
            ("open", vec![s], Some(s), vec![any]),
            ("lines", vec![], Some(any), vec![any]),
            ("close", vec![], Some(any), vec![any]),
        ]);
        self.prelude_global("io", io_lib);

        // `_G` 是全局表自己。形状无从静态说起（它的字段就是全体全局），
        // 所以给 `table<string, any>`：README 里 `_G.logger as Logger` 那一句靠它成立
        let args = self.type_arenas.intern_list(vec![s, any], None);
        let g = self.type_arenas.reference(DeclId::TABLE, args);
        self.prelude_global("_G", g);
        self.prelude_global("arg", g);
    }

    /// 造一个非泛型函数类型。`vararg` 是尾部那格可选实参的元素类型 ——
    /// 标准库里大量函数的后几个参数可省，而此处没有「可选形参」这一位，
    /// 拿 vararg 当它用：个数检查于是只管住「必给的那几个」
    fn prelude_func(
        &mut self,
        params: Vec<TypeId>,
        vararg: Option<TypeId>,
        ret: Vec<TypeId>,
    ) -> TypeId {
        let params = self.type_arenas.intern_list(params, vararg);
        let ret = self.type_arenas.intern_list(ret, None);
        self.type_arenas.func(ListId::EMPTY, params, ret)
    }

    /// 一张全是函数字段的 record（`string` / `os` / …那种库表）
    fn prelude_lib(
        &mut self,
        entries: &[(&str, Vec<TypeId>, Option<TypeId>, Vec<TypeId>)],
    ) -> TypeId {
        let mut fields = Vec::with_capacity(entries.len());
        for (name, params, vararg, ret) in entries {
            let ty = self.prelude_func(params.clone(), *vararg, ret.clone());
            fields.push(Field {
                name: self.names.intern(name),
                ty,
                default: false,
            });
        }
        self.type_arenas.record(fields)
    }

    /// 给一张已有的 record 再添几个非函数字段（`math.pi` 之类）
    fn prelude_extend(&mut self, base: TypeId, extra: &[(&str, TypeId)]) -> TypeId {
        let Types::Record(f) = self.type_arenas.get_type(base) else {
            return base;
        };
        let mut fields = self.type_arenas.fields(f).to_vec();
        for &(name, ty) in extra {
            fields.push(Field {
                name: self.names.intern(name),
                ty,
                default: false,
            });
        }
        self.type_arenas.record(fields)
    }

    /// 落一个 prelude 全局。重名不覆写（`declare_global` 自己拦），
    /// 返回值不看：这一串名字自己不重复，而它比任何用户代码都早
    fn prelude_global(&mut self, name: &str, ty: TypeId) {
        let slot = self.new_slot(ty, Span::default(), VarScope::Extern, true);
        self.declare_global(name, slot);
    }

    /// 登记一个 pub 类型。同模块同名、但**换了一条声明**才算重复导出，返回 false。
    /// 同一条声明重复登记是幂等的（类体预填的 eager 趟与主遍历各跟 check_pub 跑
    /// 一次，拿到的是同一个 DeclId），不算冲突
    pub fn export(&mut self, module: NameId, name: NameId, decl: DeclId) -> bool {
        match self.exports.get(&(module, name)) {
            Some(&existing) => existing == decl,
            None => {
                self.exports.insert((module, name), decl);
                true
            }
        }
    }

    /// `import {A} in "mod_a"` 的查表入口。没有 pub 的类型查不到
    pub fn lookup_export(&self, module: NameId, name: NameId) -> Option<DeclId> {
        self.exports.get(&(module, name)).copied()
    }

    /// 登记一个模块里的顶层类型（不管 pub 与否）。同名只认第一条，
    /// 和 hoist_top_level_types 的 declare_type 一个口径（重名诊断另报）
    pub fn declare_module_type(&mut self, module: NameId, name: NameId, decl: DeclId) {
        self.module_types.entry((module, name)).or_insert(decl);
    }

    /// 这个模块里到底有没有叫这名字的顶层类型。import 落空时：
    /// 真有、只是没 pub → 报「没 pub」；压根没有 → 报「模块没导出」
    pub fn module_has_type(&self, module: NameId, name: NameId) -> bool {
        self.module_types.contains_key(&(module, name))
    }

    /// 同上但把 DeclId 给出来。判「这条声明是不是本模块自己写的」要用它：
    /// 只问名字在不在会误判 —— 本模块自己有个 `Account`、又 import 了别处的
    /// `Account as Remote` 时，两条声明同名不同身份
    pub fn lookup_module_type(&self, module: NameId, name: NameId) -> Option<DeclId> {
        self.module_types.get(&(module, name)).copied()
    }

    /// 造一格变量。`assigned` = 声明语句本身就给了值（有初值 / 形参 / for 变量 /
    /// extern）；它和「ty 容得下 nil」任一成立，这个变量就算已初始化
    pub fn new_slot(&self, ty: TypeId, span: Span, kind: VarScope, assigned: bool) -> VarInfo {
        VarInfo {
            ty,
            inited: assigned || self.admits_nil(ty),
            decl_span: span,
            kind,
        }
    }

    /// `nil` 是不是 `ty` 的合法值。这是「无初值声明」的判据：容得下 nil 的类型
    /// 声明出来就是诚实的（那个变量此刻真的是 nil），不必等赋值
    pub fn admits_nil(&self, ty: TypeId) -> bool {
        let mut cur = ty;
        // 别名链成环该由 typedef 自己的检查报，这里只负责不挂死
        for _ in 0..64 {
            match self.type_arenas.get_type(cur) {
                // UNKNOWN 是「没算出来」，一律放过：不在推断失败之上再叠一条误报
                Types::Trival(Trival::Nil | Trival::Any | Trival::Unknown) => return true,
                // variants 已扁平化，所以 nil 在不在里面一眼就能看完
                Types::Union(l) => return self.type_arenas.list(l).fixed.contains(&TypeId::NIL),
                // 别名：剥掉再判。不代入泛型实参 —— `typedef Opt<T> = T|nil` 里的 nil
                // 是写在定义体里的，代不代入都在；class 不是别名，到这就到头
                Types::Ref { decl, .. } => match self.decls.get(decl).as_typedef() {
                    Some(t) => cur = t.target,
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    /// `sub` 的值放进 `sup` 位安不安全 —— 可赋值（子类型）判定的中枢。字段赋值、
    /// extends 一致性、泛型上界最终都问它。现在只有标量 / nil / any / never /
    /// union / intersect / record / 内建容器这些「体已算全」的类型走得到实处：
    /// class 的 fields、extends、typedef 的 target 要等各自 check_* 落地后才填得上，
    /// 那之前它们体是空的（extends=None / target=UNKNOWN），相关分支自然落到
    /// 「不可赋」或渐进放过。规则照最终形态写好，体一填就即刻生效。
    ///
    /// 几处 P0b 暂定的口径（都待维护者最终拍板）：
    ///   - Ref 的类型实参按**不变**处理（`array<never>` 不算 `array<string>` 的子型）——
    ///     容器可变，协变会破坏写位安全，先取最保守的
    ///   - record 只做宽度 + 字段协变；可选字段（default / 容 nil）的放宽暂不做
    ///   - class Ref 不对结构 record 做结构化匹配（class 是名义类型）
    ///
    /// 尺度和 `expect_key` 一致：拿不准就放过（true），把误报压到最低 —— 类型
    /// 信息还不全的阶段刷一屏假错，比漏一条难查得多
    pub fn assignable(&mut self, sub: TypeId, sup: TypeId) -> bool {
        self.assignable_within(sub, sup, 0)
    }

    fn assignable_within(&mut self, sub: TypeId, sup: TypeId, depth: u32) -> bool {
        // 哈希 consing 保证同一个类型只有一个 TypeId，自反顺带收掉递归类型的自比
        if sub == sup {
            return true;
        }
        // 别名 / extends 链成环该由各自的检查报，这里够深就放过：不挂死也不误报
        if depth >= 64 {
            return true;
        }
        // 渐进逃逸口：any 是显式的、unknown 是「还没算出来」，两向都放过
        if matches!(sub, TypeId::ANY | TypeId::UNKNOWN)
            || matches!(sup, TypeId::ANY | TypeId::UNKNOWN)
        {
            return true;
        }
        // never 是底：塞得进任何位。反向只有 never 收得下 never，已被自反收掉
        if sub == TypeId::NEVER {
            return true;
        }

        // ---- 先拆两侧的 union / intersect：复合形状要在具体形状之前判 ----
        // sub 是联合：每个分支都得进得去 sup
        if let Types::Union(l) = self.type_arenas.get_type(sub) {
            let ms = self.type_arenas.list(l).fixed.clone();
            return ms
                .iter()
                .all(|&m| self.assignable_within(m, sup, depth + 1));
        }
        // sup 是交：sub 得同时满足每个成员
        if let Types::Intersect(l) = self.type_arenas.get_type(sup) {
            let ms = self.type_arenas.list(l).fixed.clone();
            return ms
                .iter()
                .all(|&m| self.assignable_within(sub, m, depth + 1));
        }
        // sup 是联合：sub 进得了任一分支即可（sub 此时已非 union）
        if let Types::Union(l) = self.type_arenas.get_type(sup) {
            let ms = self.type_arenas.list(l).fixed.clone();
            return ms
                .iter()
                .any(|&m| self.assignable_within(sub, m, depth + 1));
        }
        // sub 是交：任一成员进得去就行（sup 此时已非 union / 非 intersect）
        if let Types::Intersect(l) = self.type_arenas.get_type(sub) {
            let ms = self.type_arenas.list(l).fixed.clone();
            return ms
                .iter()
                .any(|&m| self.assignable_within(m, sup, depth + 1));
        }

        // ---- 到这里两侧都是叶形状：Trival / Generic / Ref / Record / Func / Pack ----

        // 泛型形参：实例化前还是个未知位，拿不准能否赋就放过。上界不在这
        // 里校：它只管的是实参能不能代给形参，那归实例化点的 check_generic_bounds
        if matches!(self.type_arenas.get_type(sub), Types::Generic(_))
            || matches!(self.type_arenas.get_type(sup), Types::Generic(_))
        {
            return true;
        }

        // typedef 是透明别名：任一侧是 typedef 就展开定义体（带实参代入）再比
        if let Some(expanded) = self.expand_typedef(sub) {
            return self.assignable_within(expanded, sup, depth + 1);
        }
        if let Some(expanded) = self.expand_typedef(sup) {
            return self.assignable_within(sub, expanded, depth + 1);
        }

        match (
            self.type_arenas.get_type(sub),
            self.type_arenas.get_type(sup),
        ) {
            // 空表 `{}` 能进内建容器位：`local xs:array<string> = {}`。名义 class
            // 造不出来（得 `A{…}`），所以只对 array / table 开
            (Types::Record(f), Types::Ref { decl, .. })
                if decl.is_builtin_container() && self.type_arenas.fields(f).is_empty() =>
            {
                true
            }
            // 具名字段的表字面量进映射位：`local mod:Meta = {VERSION = 1}`。
            // README「表的形状一次定死」里，模块表想事后长新成员就得写成
            // `{VERSION:number} & table<string, …>`，那一半交就落在这一步。
            // record 的字段名天生是 string，所以键那半只问 string 进不进得了 K，
            // 值那半逐个字段比 V。只对 table 开：array 的键是下标，具名字段对不上
            (Types::Record(f), Types::Ref { decl, args }) if decl == DeclId::TABLE => {
                let fields = self.type_arenas.fields(f).to_vec();
                let kv = self.type_arenas.list(args).fixed.clone();
                let (Some(&k), Some(&v)) = (kv.first(), kv.get(1)) else {
                    return true;
                };
                self.assignable_within(TypeId::STRING, k, depth + 1)
                    && fields
                        .iter()
                        .all(|x| self.assignable_within(x.ty, v, depth + 1))
            }
            // record 宽度 + 字段协变：sup 要的每个字段 sub 都得有，且字段类型可赋
            (Types::Record(fsub), Types::Record(fsup)) => {
                let sub_fields = self.type_arenas.fields(fsub).to_vec();
                let sup_fields = self.type_arenas.fields(fsup).to_vec();
                sup_fields
                    .iter()
                    .all(|sf| match sub_fields.iter().find(|x| x.name == sf.name) {
                        Some(x) => self.assignable_within(x.ty, sf.ty, depth + 1),
                        None => false,
                    })
            }
            // 名义子型：sub 是 class，沿 extends 链上溯（父类实参先用子类的代入过），
            // 看能不能走到 sup。extends 现在恒 None，所以不同 class 现在一律不可赋
            (Types::Ref { decl, args }, _) => {
                let Some(parent) = self.class_extends(decl) else {
                    return false;
                };
                let parent = self.subst_ref_args(decl, args, parent);
                self.assignable_within(parent, sup, depth + 1)
            }
            // 函数：形参逆变、返回协变
            (
                Types::Func {
                    params: ps,
                    ret: rs,
                    ..
                },
                Types::Func {
                    params: pp,
                    ret: rp,
                    ..
                },
            ) => {
                // 形参逆变：sup 的每个参数要能进 sub 的（调用点传的是 sub 参数位）
                self.list_assignable(pp, ps, depth + 1)
                    // 返回协变：sub 的返回要能进 sup 的
                    && self.list_assignable(rs, rp, depth + 1)
            }
            // 标量之间、以及形状不匹配：只有相等才可赋，而相等已被自反收掉
            _ => false,
        }
    }

    /// `ty` 是个 typedef 引用就展开它的定义体（实参代进 target），否则 None。
    /// class 不是别名，返回 None —— 名义身份要留住。
    /// 容器形状的判定（as_array / as_map）只认 array / table 的 Ref，
    /// 所以 linter 那边也要用它先展开别名，因此是公开的
    pub fn expand_typedef(&mut self, ty: TypeId) -> Option<TypeId> {
        let Types::Ref { decl, args } = self.type_arenas.get_type(ty) else {
            return None;
        };
        let target = self.decls.get(decl).as_typedef()?.target;
        Some(self.subst_ref_args(decl, args, target))
    }

    /// class 的父类（`extends`）。不是 class 或没有父类都给 None
    fn class_extends(&self, decl: DeclId) -> Option<TypeId> {
        self.decls.get(decl).as_class()?.extends
    }

    /// `decl` 的泛型形参 -> `args` 的代入表。非泛型时是空代入，subst 恒等。
    /// 类字段存的都是本类形参写的类型，凡是从一个 Ref 往里查（字段 / 父类）
    /// 都得先拿这张表代一遍，所以它是公开的
    pub fn ref_subst(&mut self, decl: DeclId, args: ListId) -> SubstId {
        let generics = self.decls.get(decl).generics.clone();
        let arg_tys = self.type_arenas.list(args).fixed.clone();
        self.type_arenas.intern_subst(&generics, &arg_tys)
    }

    /// 把 `decl` 的泛型形参按 `args` 代进 `ty`
    fn subst_ref_args(&mut self, decl: DeclId, args: ListId, ty: TypeId) -> TypeId {
        let s = self.ref_subst(decl, args);
        self.type_arenas.subst(ty, s)
    }

    /// 列表逐位可赋：`sub_list[i]` 都能进 `sup_list[i]`，且定长个数一致。
    /// 变长位的方差由调用点通过交换实参表达，这里只做逐位 assignable
    fn list_assignable(&mut self, sub_list: ListId, sup_list: ListId, depth: u32) -> bool {
        let a = self.type_arenas.list(sub_list).clone();
        let b = self.type_arenas.list(sup_list).clone();
        if a.fixed.len() != b.fixed.len() {
            return false;
        }
        for (&x, &y) in a.fixed.iter().zip(b.fixed.iter()) {
            if !self.assignable_within(x, y, depth) {
                return false;
            }
        }
        match (a.vararg, b.vararg) {
            (None, None) => true,
            (Some(x), Some(y)) => self.assignable_within(x, y, depth),
            _ => false,
        }
    }

    /// 登记全局变量。返回 false = 这名字之前已经登记过，**而且原声明保持不变**：
    /// 「后续赋值不能背叛声明」要求第一处声明是唯一权威，覆盖掉就无从比较了
    pub fn declare_global(&mut self, name: &str, slot: VarInfo) -> bool {
        if self.globals.contains_key(name) {
            return false;
        }
        self.globals.insert(name.to_string(), slot);
        true
    }

    pub fn lookup_global(&self, name: &str) -> Option<VarInfo> {
        self.globals.get(name).copied()
    }

    /// 给全局置上「已赋值」。`None` = 这名字压根没声明过，由调用点报；
    /// `Some(flipped)` 的 flipped = 这次真把 false 推成了 true —— 定值分析的
    /// 日志只记翻转，回滚才是精确的（对本来就 inited 的变量再赋值不该留痕）
    pub fn mark_global_inited(&mut self, name: &str) -> Option<bool> {
        self.globals.get_mut(name).map(|slot| {
            let flipped = !slot.inited;
            slot.inited = true;
            flipped
        })
    }

    /// 顶层 `function f` 的类型补登记。抬升那一遍只登记名字 —— 那时函数体还没
    /// 遍历过，类型无从得知，得留到 @FuncDecl 的 leave 补上来。
    ///
    /// 只认 HoistedFn、且只从 UNKNOWN 起改：`declare_global` 守着「第一处声明是
    /// 唯一权威」，这里要是谁都能改，就等于给「后续赋值改写声明」开了后门
    pub fn patch_hoisted_type(&mut self, name: &str, ty: TypeId) {
        if let Some(slot) = self.globals.get_mut(name) {
            if slot.kind == VarScope::HoistedFn && slot.ty == TypeId::UNKNOWN {
                slot.ty = ty;
            }
        }
    }

    /// 按分支合并的结果就地改写 `inited`。和 `mark_global_inited` 分开是因为
    /// 回滚要写 false，而「赋值」这个动作只会往 true 推
    pub fn set_global_inited(&mut self, name: &str, inited: bool) {
        if let Some(slot) = self.globals.get_mut(name) {
            slot.inited = inited;
        }
    }

    /// 内建类型名的兜底查表。linter 的 scope_stack 全落空才该问到这里，
    /// 所以用户写 `class array<T>` 会遮蔽内建而不是和它冲突
    pub fn lookup_builtin_type(&self, name: &str) -> Option<TypeRef> {
        self.builtin_types.get(name).copied()
    }

    /// 打印类型。`TypeId` 现在只是个 u32，裸 Debug 没有信息量，诊断一律走这里
    pub fn show(&self, ty: TypeId) -> TypeShow<'_> {
        TypeShow { s: self, ty }
    }
}

pub struct TypeShow<'a> {
    s: &'a Session,
    ty: TypeId,
}

impl fmt::Display for TypeShow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_type(self.s, self.ty, f)
    }
}

/// Ref 只打名字和实参、从不展开定义体，所以递归 typedef 不会打印到死
fn write_type(s: &Session, ty: TypeId, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match s.type_arenas.get_type(ty) {
        Types::Trival(p) => f.write_str(match p {
            Trival::Unknown => "unknown",
            Trival::Any => "any",
            Trival::Nil => "nil",
            Trival::Boolean => "boolean",
            Trival::Number => "number",
            Trival::String => "string",
            Trival::Never => "never",
        }),
        Types::Generic(g) => f.write_str(s.names.resolve(s.decls.generic(g).name())),
        Types::Ref { decl, args } => {
            f.write_str(s.names.resolve(s.decls.get(decl).name()))?;
            let list = s.type_arenas.list(args);
            if list.is_empty() {
                return Ok(());
            }
            f.write_str("<")?;
            write_seq(s, args, f)?;
            f.write_str(">")
        }
        Types::Union(l) => {
            for (i, &v) in s.type_arenas.list(l).fixed.iter().enumerate() {
                if i > 0 {
                    f.write_str(" | ")?;
                }
                write_type(s, v, f)?;
            }
            Ok(())
        }
        Types::Intersect(l) => {
            for (i, &v) in s.type_arenas.list(l).fixed.iter().enumerate() {
                if i > 0 {
                    f.write_str(" & ")?;
                }
                write_type(s, v, f)?;
            }
            Ok(())
        }
        Types::Record(fields) => {
            f.write_str("{")?;
            for (i, fld) in s.type_arenas.fields(fields).iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{}:", s.names.resolve(fld.name))?;
                write_type(s, fld.ty, f)?;
            }
            f.write_str("}")
        }
        Types::Func {
            generics,
            params,
            ret,
        } => {
            f.write_str("function")?;
            // 泛型函数把形参表一并印出来：光看 `function(T) -> T` 分不出 T 是它
            // 自己的形参还是外层 class 的
            if !s.type_arenas.list(generics).fixed.is_empty() {
                f.write_str("<")?;
                write_seq(s, generics, f)?;
                f.write_str(">")?;
            }
            f.write_str("(")?;
            write_seq(s, params, f)?;
            f.write_str(") -> ")?;
            let r = s.type_arenas.list(ret);
            if r.fixed.len() == 1 && r.vararg.is_none() {
                write_type(s, r.fixed[0], f)
            } else {
                f.write_str("(")?;
                write_seq(s, ret, f)?;
                f.write_str(")")
            }
        }
        Types::Pack(l) => {
            f.write_str("(")?;
            write_seq(s, l, f)?;
            f.write_str(")")
        }
    }
}

fn write_seq(s: &Session, l: ListId, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let list = s.type_arenas.list(l);
    for (i, &t) in list.fixed.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write_type(s, t, f)?;
    }
    if let Some(v) = list.vararg {
        if !list.fixed.is_empty() {
            f.write_str(", ")?;
        }
        f.write_str("...")?;
        write_type(s, v, f)?;
    }
    Ok(())
}

#[cfg(test)]
mod assignable_tests {
    //! P0b `assignable` 的单测。直接拿 arena 造类型再问可赋，不走 parser：
    //! 目前只有体已算全的那些类型（标量 / nil / any / never / union /
    //! intersect / record / 内建容器）走得到实处。typedef 展开那一条手工填一下
    //! target，提前验一下 P1 落地后的行为
    use super::*;
    use crate::lexer::type_def::Span;

    fn field(s: &mut Session, name: &str, ty: TypeId) -> Field {
        Field {
            name: s.names.intern(name),
            ty,
            default: false,
        }
    }

    fn array_of(s: &mut Session, elem: TypeId) -> TypeId {
        let args = s.type_arenas.intern_list(vec![elem], None);
        s.type_arenas.reference(DeclId::ARRAY, args)
    }

    fn table_of(s: &mut Session, k: TypeId, v: TypeId) -> TypeId {
        let args = s.type_arenas.intern_list(vec![k, v], None);
        s.type_arenas.reference(DeclId::TABLE, args)
    }

    /// 自反 + 标量之间不相容
    #[test]
    fn reflexive_and_scalars() {
        let mut s = Session::new();
        assert!(s.assignable(TypeId::NUMBER, TypeId::NUMBER));
        assert!(!s.assignable(TypeId::NUMBER, TypeId::STRING));
        assert!(!s.assignable(TypeId::NIL, TypeId::NUMBER));
        assert!(!s.assignable(TypeId::BOOLEAN, TypeId::NUMBER));
    }

    /// any / unknown 是渐进逃逸口，两向都放过
    #[test]
    fn gradual_any_and_unknown_both_ways() {
        let mut s = Session::new();
        assert!(s.assignable(TypeId::NUMBER, TypeId::ANY));
        assert!(s.assignable(TypeId::ANY, TypeId::NUMBER));
        assert!(s.assignable(TypeId::STRING, TypeId::UNKNOWN));
        assert!(s.assignable(TypeId::UNKNOWN, TypeId::STRING));
    }

    /// never 是底：进得了任何位；反向只有 never 收得下 never
    #[test]
    fn never_is_bottom() {
        let mut s = Session::new();
        assert!(s.assignable(TypeId::NEVER, TypeId::NUMBER));
        assert!(s.assignable(TypeId::NEVER, TypeId::STRING));
        assert!(s.assignable(TypeId::NEVER, TypeId::NEVER));
        assert!(!s.assignable(TypeId::NUMBER, TypeId::NEVER));
    }

    /// nil / 标量 进可选联合 `number | nil`
    #[test]
    fn into_optional_union() {
        let mut s = Session::new();
        let opt = s.type_arenas.union(vec![TypeId::NUMBER, TypeId::NIL]);
        assert!(s.assignable(TypeId::NIL, opt));
        assert!(s.assignable(TypeId::NUMBER, opt));
        assert!(!s.assignable(TypeId::STRING, opt));
    }

    /// sub 是联合：每个分支都得进得去 sup
    #[test]
    fn union_sub_requires_every_member() {
        let mut s = Session::new();
        let ns = s.type_arenas.union(vec![TypeId::NUMBER, TypeId::STRING]);
        let nsn = s
            .type_arenas
            .union(vec![TypeId::NUMBER, TypeId::STRING, TypeId::NIL]);
        // number|string 进 number|string|nil：两个分支都能落到右边某个分支
        assert!(s.assignable(ns, nsn));
        // number|string 进 number：string 进不去
        assert!(!s.assignable(ns, TypeId::NUMBER));
    }

    /// 交类型：sup 交要同时满足每个成员、sub 交任一成员满足即可。
    /// 用 record & array——两个 record 会被 intersect() 当场并成一个，这里要的是
    /// 真正的 Intersect 变体，所以拿一个不可约的容器成员配上 record
    #[test]
    fn intersect_all_and_some() {
        let mut s = Session::new();
        let fa = field(&mut s, "a", TypeId::NUMBER);
        let rec = s.type_arenas.record(vec![fa]);
        let arr = array_of(&mut s, TypeId::NUMBER);
        let inter = s.type_arenas.intersect(vec![rec, arr]);

        // sub 是交：任一成员能进就行
        assert!(s.assignable(inter, rec));
        assert!(s.assignable(inter, arr));
        // sup 是交：光有 array 满足不了 record 那个成员
        assert!(!s.assignable(arr, inter));
        // 交自己进自己（自反）
        assert!(s.assignable(inter, inter));
    }

    /// record 宽度子型：sub 可以多字段，不能少 sup 要的字段
    #[test]
    fn record_width() {
        let mut s = Session::new();
        let fa = field(&mut s, "a", TypeId::NUMBER);
        let fb = field(&mut s, "b", TypeId::STRING);
        let narrow = s.type_arenas.record(vec![fa]);
        let wide = s.type_arenas.record(vec![fa, fb]);
        assert!(s.assignable(wide, narrow));
        assert!(!s.assignable(narrow, wide));
    }

    /// record 字段协变：`{a:number}` 进 `{a:number|nil}`，反之不行
    #[test]
    fn record_field_covariant() {
        let mut s = Session::new();
        let opt = s.type_arenas.union(vec![TypeId::NUMBER, TypeId::NIL]);
        let fa = field(&mut s, "a", TypeId::NUMBER);
        let fa_opt = field(&mut s, "a", opt);
        let strict = s.type_arenas.record(vec![fa]);
        let loose = s.type_arenas.record(vec![fa_opt]);
        assert!(s.assignable(strict, loose));
        assert!(!s.assignable(loose, strict));
    }

    /// 内建容器实参不变：`array<number>` 与 `array<string>` 不相容，同实参才相等
    #[test]
    fn container_args_are_invariant() {
        let mut s = Session::new();
        let arr_n = array_of(&mut s, TypeId::NUMBER);
        let arr_s = array_of(&mut s, TypeId::STRING);
        assert!(s.assignable(arr_n, arr_n));
        assert!(!s.assignable(arr_n, arr_s));
        let tab = table_of(&mut s, TypeId::STRING, TypeId::NUMBER);
        assert!(s.assignable(tab, tab));
    }

    /// 空表 `{}` 能进内建容器位；非空 record 不行
    #[test]
    fn empty_table_into_container() {
        let mut s = Session::new();
        let empty = s.type_arenas.record(vec![]);
        let arr_s = array_of(&mut s, TypeId::STRING);
        let tab = table_of(&mut s, TypeId::STRING, TypeId::NUMBER);
        assert!(s.assignable(empty, arr_s));
        assert!(s.assignable(empty, tab));
        // 非空 record 进不了容器（它不是数组/表）
        let fa = field(&mut s, "a", TypeId::NUMBER);
        let rec = s.type_arenas.record(vec![fa]);
        assert!(!s.assignable(rec, arr_s));
    }

    /// typedef 是透明别名：手工填上 `typedef Count = number` 的 target，
    /// 验一下 P1 落地后 number 与 Count 两向相容、string 不行
    #[test]
    fn typedef_is_transparent() {
        let mut s = Session::new();
        let count_name = s.names.intern("Count");
        let decl = s.decls.declare_typedef(count_name, Span::default());
        s.decls.get_mut(decl).as_typedef_mut().unwrap().target = TypeId::NUMBER;
        let count = s.type_arenas.reference(decl, ListId::EMPTY);
        assert!(s.assignable(TypeId::NUMBER, count));
        assert!(s.assignable(count, TypeId::NUMBER));
        assert!(!s.assignable(TypeId::STRING, count));
    }
}
