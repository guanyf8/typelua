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

/// 一个被 intern 的标识符
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// 标量与两个伪类型。**没有 void**：`-> ()` 是空 Pack，见 `TypeId::VOID`
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
    Ref { decl: DeclId, args: ListId },
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
    Func { params: ListId, ret: ListId },
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
    /// `-> ()`：零返回值，也就是空 Pack。
    /// README 里「`()` 永远不是 type」说的是**语法层**（不进 basictype，否则
    /// `-> ()` 歧义）；语义层的空值列表只能由 retspec / argtypes 产出，不歧义
    pub const VOID: TypeId = TypeId(6);

    pub const fn of_trival(p: Trival) -> TypeId {
        match p {
            Trival::Unknown => TypeId::UNKNOWN,
            Trival::Any => TypeId::ANY,
            Trival::Nil => TypeId::NIL,
            Trival::Boolean => TypeId::BOOLEAN,
            Trival::Number => TypeId::NUMBER,
            Trival::String => TypeId::STRING,
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

/// 声明层的字段：类型层那份 + 源码位置
#[derive(Debug, Clone, Copy)]
pub struct ClassField {
    pub field: Field,
    pub span: Span,
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
        ] {
            a.intern(Types::Trival(p));
        }
        let void = a.intern(Types::Pack(ListId::EMPTY));

        debug_assert_eq!(empty_list, ListId::EMPTY);
        debug_assert_eq!(empty_fields, FieldsId::EMPTY);
        debug_assert_eq!(a.intern(Types::Trival(Trival::Unknown)), TypeId::UNKNOWN);
        debug_assert_eq!(a.intern(Types::Trival(Trival::String)), TypeId::STRING);
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
            Types::Func { params, ret } => {
                self.list_generic[params.idx()] || self.list_generic[ret.idx()]
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
            Types::Func { params, ret } => {
                self.list_conflict(params).or_else(|| self.list_conflict(ret))
            }
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

    pub fn func(&mut self, params: ListId, ret: ListId) -> TypeId {
        self.intern(Types::Func { params, ret })
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
            Types::Func { params, ret } => {
                let params = self.subst_list(params, s);
                let ret = self.subst_list(ret, s);
                self.func(params, ret)
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
    /// 全局变量表。全局是 `_ENV` 的字段、整个工程同一份，所以它跟着
    /// Session 攒，不进 per-file 重置的 scope_stack（README §15：全局变量
    /// 的标注得收进工程级命名空间，否则别的文件看不到 `G_config` 的类型）。
    /// linter 查变量时把它当作作用域链的最外一环兜底，不复制进 scope_stack：
    /// 只有一份才不会不一致。
    ///
    /// 键用 String 而不是 NameId：消费者是 linter 那边 String 键的作用域表，
    /// 而且 `&self` 的查表位拿不到 `&mut names`，没法顺手 intern
    globals: HashMap<String, TypeId>,
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
            globals: HashMap::new(),
            builtin_types: HashMap::new(),
        };
        session.register_builtins();
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

    /// 登记一个 pub 类型。同模块同名重复导出返回 false
    pub fn export(&mut self, module: NameId, name: NameId, decl: DeclId) -> bool {
        if self.exports.contains_key(&(module, name)) {
            return false;
        }
        self.exports.insert((module, name), decl);
        true
    }

    /// `import {A} in "mod_a"` 的查表入口。没有 pub 的类型查不到
    pub fn lookup_export(&self, module: NameId, name: NameId) -> Option<DeclId> {
        self.exports.get(&(module, name)).copied()
    }

    /// 登记全局变量的类型。返回 false = 这名字之前已经登记过（标注照样
    /// 覆盖：同一个全局在两处给出不同标注算不算错，由 linter 定）
    pub fn declare_global(&mut self, name: &str, ty: TypeId) -> bool {
        self.globals.insert(name.to_string(), ty).is_none()
    }

    pub fn lookup_global(&self, name: &str) -> Option<TypeId> {
        self.globals.get(name).copied()
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
        Types::Func { params, ret } => {
            f.write_str("function(")?;
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
