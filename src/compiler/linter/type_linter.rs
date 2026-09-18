//! 类型 linter：属性求值 + 诊断。
//!
//! 分节导航（改动前先认准落在哪一节）：
//!   §1 节点属性   node_values 的存取，以及「多值截一个 / 取第 i 个值」这类
//!                 只关乎属性本身的归一
//!   §2 CST 形状   按形状认节点。全是只读 ast 的纯查询，不碰 linter 状态
//!   §3 名字表     作用域栈：变量与块级类型名。栈内落空才去问 Session
//!   §4 定值分析   流帧 / 分支合并 / 可信度降级
//!   §5 分派       `resolve` 与 `resolve_prod`：本文件的目录，一个标签一行
//!   §6 表达式     运算、索引、字段查找、调用、变量读
//!   §7 表构造     值位表字面量与类型位匿名 record 共用的字段收集
//!   §8 类型位     类型名 / 泛型实例化 / 联合 / 交 / 列表打包
//!   §9 声明与语句 local / 全局赋值 / extern / function，以及驱动点要用的动作
//!   §10 类与方法   class 子系统：声明填体、继承、字段，以及类体内外的方法定义
//!   §11 跨模块    import / pub：只搬「类型名 -> DeclId」，纯编译期
//!   §12 占位      语义不在节点本身上的产生式，一律返回 UNKNOWN
//!   驱动          `impl Linter`：前序开作用域与流帧、后序求值与收口
//!
//! 文件太大，按上面的分节拆了几块到同名子模块（都还是 `impl TypeLinter` 的
//! 内在方法，标 pub(super) 给父模块和兄弟节调用）：§4→flow.rs、§6→expr.rs、
//! §7→table.rs、§8→type_pos.rs、§9→decl_stmt.rs、§10→class.rs、§11→cross_module.rs。
//! §1/§2/§3/§5/§12 与驱动仍留在本文件。
//!
//! 两条贯穿全文的约定：
//!   - **求值在后序**（`leave`）：孩子的类型先算好才谈得上组合。要在前序做的
//!     事只有三类，且各自在原处写明了为什么（作用域/流帧的「进」、@VarRef 的
//!     两条诊断、`local function` 的名字预声明）
//!   - `resolve_*` 出类型、`check_*` 出诊断。前者的返回值会被登记进
//!     `node_values` 供父节点读，后者只往 log 里写

// 属性只有一种值：`TypeId`。类型列表也是一等的（`Types::Pack(ListId)`），装进去用
// `type_arenas.pack(..)`、拆出来用 `type_arenas.as_list(..)`，中间只走 TypeId 一条道
use super::*;
use crate::compiler::utils::string_literal;
use crate::lexer::type_def::*;
use crate::parser::ast::Tree;
use crate::parser::parser::*;
use crate::parser::table::*;
use std::collections::HashMap;
use std::collections::HashSet;

// TypeLinter 的 impl 按 §分节拆到这些子模块里：都是 `impl TypeLinter` 的
// 内在方法，跨文件互调不用 import；方法在子模块里标 pub(super)，父模块的分派
// （§5）与各兄弟节才看得见。子模块靠 `use super::*` 取到本文件的私有辅助类型
mod class; // §10 类与方法
mod decl_and_stat; // §9 声明与语句
mod expr; // §6 表达式
mod flow; // §4 定值分析
mod import; // §11 import / pub
mod table; // §7 表构造
mod type_note; // §8 类型位
mod types;

use types::*;

struct Scope {
    /// 变量位。存 `VarSlot` 而不是裸 `TypeId`：定值分析要那一位 `inited`，
    /// 「赋值背叛声明」的诊断要 `decl_span`
    variables: HashMap<String, VarInfo>,
    /// 本块入口处的 nil 收窄（`if s ~= nil then` 的体内 s 不再含 nil）。
    /// 和 `variables` 分两张表放：它**不是一次声明** —— 同层再 `local s`
    /// 不算重复声明，而变量的归属（哪一层、inited 没没）也还归原那格管
    narrowed: HashMap<String, TypeId>,
    type_names: HashMap<String, TypeRef>, // 块级类型名：声明时注册，内层遮蔽外层
}

impl Scope {
    fn new() -> Self {
        Scope {
            variables: HashMap::new(),
            narrowed: HashMap::new(),
            type_names: HashMap::new(),
        }
    }
}

/// 一次「`inited` 从 false 推到 true」的翻转。回滚和取交集都按它走。
///
/// 光记名字不行：分支里 `local n:number` 遮蔽外层同名变量、再给它赋值时，
/// 按名字回滚会打到外层那一个。得把「当时打的是哪一格」一起记下来
#[derive(Clone, PartialEq)]
struct InitFlip {
    name: String,
    /// 置位落在 `scope_stack` 的第几层。`None` = Session 里的全局
    scope: Option<usize>,
}

/// 一个「直线区域」的帧：文件本体、一条分支块、或一个函数体。
/// 开着的时候是它，关掉之后结果是 `BranchExit`
struct FlowFrame {
    /// 进来时 `init_record` 的长度。本帧内的翻转全在这之后
    record_mark: usize,
    /// 进来时作用域栈的深度。比它深的翻转落在离开时已经弹掉的作用域里，
    /// 那些变量出了本帧就不存在，既不回滚也不参与交集
    scope_depth: usize,
    /// 已经 `return` / `break` / `goto` / 调过不返回的函数：后面的语句到不了，
    /// 这一支也就不参与交集。`if c then n=1 else return end` 就靠它走通
    terminated: bool,
    /// 本帧已经报过一条「到不了」。不可达是成段出现的，逐条报就是一屏；
    /// 而「已经报过」是本帧的属性不是全局的 —— 内层块报过不影响外层再报
    dead_reported: bool,
    /// 帧内出现过 `goto` / label：向后跳让线性合并失效（那才需要定点迭代），
    /// 于是这整段的 inited 检查降级为放过：宁可漏报，不误报
    untrusted: bool,
    /// 这一格是函数体（文件 chunk 也算，Lua 里它就是个函数）。它同时是两件事
    /// 的边界：`untrusted` 往外传到这里为止（嵌套闭包里的一个 `goto` 不应该
    /// 把整个文件的检查关掉），`return` 也终结不了它外面那层
    is_fn_body: bool,
}

/// 一个函数体的返回信息。进 funcbody / methoddef 压一格，出体弹掉；
/// 嵌套闭包各占一格，所以是栈
struct RetFrame {
    /// 体那个节点。leave 里认下标才敢弹，也是「推断出来的返回类型归谁」的钥匙
    node: usize,
    /// 带 rettype 槽的那个节点：funcbody 是它自己，@MethodDef 是它的 methodsig。
    /// 标注的返回类型要从这儿取
    sig: usize,
    /// 写了 `->` 没有。**形状**在前序就看得出来，而类型得等 rettype 那棵子树求完
    annotated: bool,
    /// 函数自己的名字，`function f` / `local function f` 才有。体里读到它
    /// 而又没标注返回类型，就是「递归函数必须标注返回类型」
    name: Option<NameId>,
    /// 体里见过的 return 值，每条 return 一项。没标注时靠它们推返回类型
    seen: Vec<TypeId>,
    /// 「递归须标注」已经报过：一个体里可能有好几处自引用，报一次就够
    reported: bool,
}

/// 条件里那一句 nil 测试的形状。三种的区别在「反面能不能用」：
/// `s ~= nil` / `s == nil` 两面都确切，而裸 `if s then` 只正面确切 ——
/// 落空那一支里 s 可能是 false，那不能推出它是 nil
#[derive(Clone, Copy)]
enum NilTest {
    /// `s ~= nil`
    NotNil,
    /// `s == nil`
    IsNil,
    /// 裸名字当条件：`if s then`
    Truthy,
}

/// 一条分支跑完的结果：`FlowFrame` 关掉之后留下的那些
struct BranchExit {
    /// 本分支里翻转过的位。只记翻转，所以天然去重，交集就是一次 `contains`
    flips: Vec<InitFlip>,
    terminated: bool,
}

/// 各分支的置位怎么合进外层帧。三条路子一一对应 Lua 的语句种类，分的是
/// 「这些分支盖住了多少路径」而**不是分支数** —— `do … end` 和没 else 的
/// `if` 都只有一个块，却分属 `Always` 和 `Skippable`
#[derive(Clone, Copy, PartialEq)]
enum JoinMode {
    /// 分支穷尽了所有路径（`if … else … end`）：非终结分支取交集；
    /// 一条都出不来就连外层一起终结
    Exhaustive,
    /// 还有一条什么都不做的落空路径，分支可能整个被跳过：交集必然为空，
    /// 一律丢弃。没有 else 的 `if`、可能 0 次的 `while`/`for`、**以及函数体**
    /// （不知道会不会被调）在定值分析里是同一件事，共用这一支
    Skippable,
    /// 那唯一一条分支必定执行（`do … end`、`repeat` 的体）：原样应用
    Always,
}

/// 表字面量里一个元素的三种去处。位置字段和动态键都成不了具名 `Field`
/// （`Field.name` 要 `NameId`），但它们的键/值类型算得出来 —— 所以不能像
/// 只能出 record 时那样一律丢掉，得分桶押给 `resolve_fields` 做聚合判定
enum Elem {
    /// 无键（`{1,2}`）或键是 number（`{[1]=v}` / `{[i]=v}`）—— 都是数组形态
    Positional(TypeId),
    /// 静态具名：NAME 键、字面量 string 键、类型位的 FieldDecl / 方法
    Named(Field),
    /// 动态键 `[k]=v`：k 既不是 number 也不是字面量 string
    Dynamic { key: TypeId, val: TypeId },
}

/// 沿 extends 链拍平后的一个类字段（类型已代入实参）。`Field` 差一位
/// `method` —— 那位在声明层的 `ClassField` 上、故意不进类型层，而构造检查
/// 恰恰要靠它分路（方法不许在构造里给），所以单独成一个类型
#[derive(Clone, Copy)]
struct FlatField {
    name: NameId,
    ty: TypeId,
    /// 声明里带了默认值：构造时可省
    default: bool,
    /// 方法（只有 @MethodDef 算），不是函数变量字段
    method: bool,
}

/// 字段列表的两个来源。同一个 `resolve_fields` 服务两边，但重名的处理相反，
/// 所以得把来源带下去：值位按 Lua 语义「后写覆盖先写」，类型位重名是笔误要报错
#[derive(Clone, Copy, PartialEq)]
enum FieldSite {
    /// `'{' fieldlist '}'`，值位的表字面量
    Literal,
    /// `'{' classfieldlist '}'`，类型位的匿名 record
    Record,
}

/// 元方法命中之后，结果类型怎么定
#[derive(Clone, Copy)]
enum MetaResult {
    /// 算术、位运算、`..`：按「同类型运算」的约定返回操作数自己的类型
    SameAsOperand,
    /// 比较：`a < b` 无论走不走 `__lt` 结果都是 boolean，和操作数类型无关
    Fixed,
}

/// 运算符 -> (元方法名, 结果怎么定)。`None` 表示这个运算符不查元表。
///
/// `-` 和 `~` 一元二元都有，而且元方法不同（`__sub`/`__unm`、`__bxor`/`__bnot`），
/// 所以**必须**靠 arity 分开。原来那张一元二元合并的表两种都只出 NUMBER，掩盖了
/// 这个区别，连注释都写着「一元和二元的 token 集不相交」—— 那句话对 `-` 和 `~` 不成立
///
/// 不查元表的几类：
///   - `and`/`or`/`not`：Lua 没有对应元方法
///   - `==`/`~=`：任意两个值都能比，`__eq` 只在两侧都是表且原始不等时才被咨询，
///     结果照样是 boolean。要求它存在会把 `a == nil` 这种正常写法拦掉
///   - `#`：Lua 里**任何表**都能取长度，不需要 `__len`（`__len` 只是用来改写默认行为）。
///     要求它存在会把 `#arr` 判成错，是实打实的误报
fn metamethod(op: &Token, unary: bool) -> Option<(&'static str, MetaResult)> {
    use MetaResult::{Fixed, SameAsOperand};
    let hit = match op {
        // 一元的两个先拦，否则会被下面的二元同字符分支吃掉
        Token::OPERATOR(OpType::SIMPLE('-')) if unary => ("__unm", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('~')) if unary => ("__bnot", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('+')) => ("__add", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('-')) => ("__sub", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('*')) => ("__mul", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('/')) => ("__div", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('%')) => ("__mod", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('^')) => ("__pow", SameAsOperand),
        Token::OPERATOR(OpType::IDIV) => ("__idiv", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('&')) => ("__band", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('|')) => ("__bor", SameAsOperand),
        Token::OPERATOR(OpType::SIMPLE('~')) => ("__bxor", SameAsOperand),
        Token::OPERATOR(OpType::SHL) => ("__shl", SameAsOperand),
        Token::OPERATOR(OpType::SHR) => ("__shr", SameAsOperand),
        Token::OPERATOR(OpType::CONCAT) => ("__concat", SameAsOperand),
        // `>` / `>=` 在 Lua 里是把操作数交换后走 `__lt` / `__le`，没有独立元方法
        Token::OPERATOR(OpType::SIMPLE('<' | '>')) => ("__lt", Fixed),
        Token::OPERATOR(OpType::LE | OpType::GE) => ("__le", Fixed),
        _ => return None,
    };
    Some(hit)
}

pub struct TypeLinter {
    // 这里都是一些要传递继承属性而存在的缓冲区，最理想的状况是
    // 这里什么也没有，可以从子节点自然继承
    //todo node_index 本身是稠密的 arena 下标，Tree 暴露节点总数后这里能换成 Vec<TypeId>
    node_values: HashMap<usize, TypeId>,
    scope_stack: Vec<Scope>,
    /// 定值分析的置位日志。只记**翻转**（false -> true），所以回滚是精确的
    init_record: Vec<InitFlip>,
    /// 流帧栈。栈底那格是 `flow_reset` 压的「文件帧」，于是「当前帧」
    /// 永远存在，`flow_*` 不必到处判空
    flow_stack: Vec<FlowFrame>,
    /// 分支语句栈。一条 `if` / 循环 / `do` 进来占一格，各分支的结果攒在里面。
    /// 和 `flow_stack` 分开是因为 `elseif` 在 CST 里是左递归嵌套的（后一个
    /// @ElseIf 包着前一个），各分支块并不是兄弟，攒在节点上攒不起来
    join_stack: Vec<Vec<BranchExit>>,
    /// 本文件里 hoist 出来的 class/typedef DeclId。DeclTable 跨文件累积，
    /// 字段覆盖的收口（finish）只该扫本文件这几条，所以单列、prepare 里清
    file_decls: Vec<DeclId>,
    /// 正在填体的 class 的 Ref 类型（进类体 push、出类体 pop）。methodsig 里
    /// 不标注的 self 靠栈顶取默认类型；嵌套的 class 也能叠，所以是栈不是单格
    class_stack: Vec<TypeId>,
    /// 函数体栈。返回值核对、无标注时的返回类型推断、以及「递归须标注」
    /// 都靠栈顶那格，见 `RetFrame`
    ret_stack: Vec<RetFrame>,
    /// 旧的 File + Global 合并进 Session：名字/类型/声明/导出都在这一层。
    ///
    /// **自持,不外借**。共享的只是类型，这件事不该被 linter 层感知：`&mut Session`
    /// 得由搭 LintDriver 的那一层提供，会逼着通用 lint 层认识类型系统，还和
    /// `Box<dyn Linter>` 的 'static 打架、独占借用挡住 emitter。
    ///
    /// 跨文件累积靠的是**粒度**：TypeLinter 的粒度是「类型阶段」而非「一个文件」，
    /// 同一个实例被驱动着依次跑过各文件，Session 自然攒起来。per-file 的只有
    /// node_values / scope_stack，该在 prepare() 里重置。
    ///
    /// 这也是 `ast` 不当字段、而是每个节点方法都吃一个参数的原因：存成字段
    /// 就把实例绑在一颗树上，`prepare` 拿到的那个 `&Tree` 生命周期更短、存不回去，
    /// 跨文件累积当场矛盾；附带好处是 TypeLinter 本身变成 'static，
    /// 不再和 `Box<dyn Linter>` 打架
    session: Session,
    /// 当前文件所属模块名，`prepare` 里由驱动传进来的 module 字串 intern 而成。
    /// pub 拿它当导出的键一半，跨文件的 import 靠它知道自己往哪个模块查
    current_module: NameId,
}

impl TypeLinter {
    pub fn new() -> Self {
        let mut session = Session::new();
        // 占个默认模块名：真正的名字 prepare 每换一次文件就重置一次，
        // 但未经 prepare 就被直接戳的路径（如单独单元测）不能拿到未初始化的 NameId
        let current_module = session.names.intern("");
        TypeLinter {
            node_values: HashMap::new(),
            scope_stack: Vec::new(),
            init_record: Vec::new(),
            flow_stack: Vec::new(),
            join_stack: Vec::new(),
            file_decls: Vec::new(),
            class_stack: Vec::new(),
            ret_stack: Vec::new(),
            session,
            current_module,
        }
    }

    // ================== §1 节点属性 ==================

    fn register_value(&mut self, node_index: usize, ty: TypeId) {
        if self.node_values.contains_key(&node_index) {
            panic!("reentry node {} not allowed", node_index)
        }
        self.node_values.insert(node_index, ty);
    }

    /// 取子节点已注册的类型。没注册（标点符号、未实现的分支）算 UNKNOWN
    #[inline]
    fn child_type(&self, node_index: usize) -> TypeId {
        self.node_values
            .get(&node_index)
            .copied()
            .unwrap_or(TypeId::UNKNOWN)
    }

    /// 取第 `child` 个孩子的类型。`'(' x ')'`、`ARROW x`、`NAME ':' x` 这类
    /// 「标点包着一个真家伙」的产生式全是这个形状
    #[inline]
    fn pass_through(&self, ast: &Tree<NodeSyntax<'_>>, node_index: usize, child: usize) -> TypeId {
        self.child_type(ast.get_node(node_index).children[child])
    }

    /// `optype : /*empty*/ | ':' type` 的标注。空产生式不可折叠，所以「没标注」
    /// 是个零孩子的节点而不是缺孩子。按**形状**判而不是按类型判：
    /// 标注成什么都算标注过，就算它没推出来而是 UNKNOWN
    fn type_annotation(&self, ast: &Tree<NodeSyntax<'_>>, optype: usize) -> Option<TypeId> {
        if ast.get_node(optype).children.is_empty() {
            return None;
        }
        Some(self.child_type(optype))
    }

    /// 多值截成单值：非末位的多值表达式、以及 `(f())` 都只留第一个值。
    /// 一个值都不产出（`-> ()`）时是 nil —— Lua 里少给的实参就是 nil
    fn truncate_ret(&self, ty: TypeId) -> TypeId {
        match self.session.type_arenas.get_type(ty) {
            Types::Pack(l) => {
                let list = self.session.type_arenas.list(l);
                // 只有 vararg 的 Pack（`-> ...T`）截出来是它的元素类型
                list.fixed
                    .first()
                    .copied()
                    .or(list.vararg)
                    .unwrap_or(TypeId::NIL)
            }
            _ => ty,
        }
    }

    /// explist 的第 `i` 个值。多于一个表达式时脊顶已经摊平成 Pack（末位展开、
    /// 其余截断都在 `resolve_list` 里做完了），所以这里只是取下标；
    /// 单个表达式不进 Pack，但它自己可能就是多值（`local a,b = f()`），
    /// 所以先判 Pack 再兑底。
    /// `None` = 名字比值多（`local a, b = 1`），那个位置没人给值
    fn listnode_value_at(&self, values: TypeId, i: usize) -> Option<TypeId> {
        match self.session.type_arenas.get_type(values) {
            Types::Pack(l) => {
                let list = self.session.type_arenas.list(l);
                list.fixed.get(i).copied().or(list.vararg)
            }
            _ => (i == 0).then_some(values),
        }
    }

    // ================== §2 CST 形状 ==================
    //
    // 全是只读 ast 的纯查询：驱动点、声明处理、列表摊平都要按形状认节点，
    // 一处写一遍。它们不碰 linter 状态，所以放在最前面当地基

    /// 这个节点上的 NAME 字面。返回的 `&str` 借的是 **ast** 而不是 self，
    /// 所以拿到名字之后能接着 `&mut self` 去声明它
    fn get_name<'t>(&self, ast: &'t Tree<NodeSyntax<'t>>, node: usize) -> Option<&'t str> {
        match ast.get_node(node).get_data() {
            NodeSyntax::Token(Token::NAME(name)) => Some(name),
            _ => None,
        }
    }

    /// 第 `i` 个孩子上的 NAME 字面。`local x` / `param` / `extern` / `for` 控制变量
    /// 这些名字位在文法里都是固定下标的裸 NAME
    fn get_child_name<'t>(
        &self,
        ast: &'t Tree<NodeSyntax<'t>>,
        node: usize,
        i: usize,
    ) -> Option<&'t str> {
        self.get_name(ast, *ast.get_node(node).children.get(i)?)
    }

    /// 名字位上的 NAME 字面，取不到就炸。`what` 只进 panic 文案。
    /// 这几处（类型名、字段名、方法名、变量名）都是文法保证的不变式，
    /// 静静返回 None 会让一个畸形节点悄悄改变语义，比当场炸难查得多
    fn name_or_panic<'t>(&self, ast: &'t Tree<NodeSyntax<'t>>, node: usize, what: &str) -> &'t str {
        self.get_name(ast, node)
            .unwrap_or_else(|| panic!("{what} must be identifier"))
    }

    /// 这个节点的产生式标签。token 叶子和无标签产生式都给 `None` ——
    /// 按形状认节点的地方只关心「是不是我要找的那个标签」，
    /// 摊开 `NodeSyntax` 只会让判断埋在两层模式里。
    /// 要分辨「无标签产生式」与「token」的地方（只有 `spine`）自己去看 get_data
    fn get_production(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> Option<Prod> {
        match ast.get_node(node).get_data() {
            NodeSyntax::Prod(prod) => *prod,
            NodeSyntax::Token(_) => None,
        }
    }

    /// 这个 @VarRef 是赋值目标而不是读。`var : prefixexp optype @Var` 只出现在
    /// varlist 里，而 `a.b = 1` / `a[i] = 1` 的 @VarRef 挂在 @Dot / @Index 底下，
    /// 所以「父节点是 @Var」恰好等价于「裸名字的写位」—— 写位是声明点或赋值点，
    /// 都不该按「读一个变量」去查表和报诊断
    fn is_writing(&self, ast: &Tree<NodeSyntax<'_>>, node_index: usize) -> bool {
        let Some(parent) = ast.get_node(node_index).parent else {
            return false;
        };
        self.get_production(ast, parent) == Some(Prod::Var)
    }

    /// 这条语句在不在 chunk 的**直接**语句列表里。全局的声明点（带标注的
    /// 全局赋值、`extern`、顶层 `function`）只认这一层：`if c then G = 1 end`
    /// 里那个 G 执行不执行取决于运行时，拿它当声明点就等于让类型表跟着控制流飘。
    ///
    /// `chunk : block` 是无标签单孩子，被 parser 折叠掉了，所以根就是那个 @Block
    fn is_chunk_top(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> bool {
        let mut cur = node;
        while let Some(parent) = ast.get_node(cur).parent {
            match self.get_production(ast, parent) {
                Some(Prod::Block) => return ast.get_node(parent).parent.is_none(),
                // stat_list 的脊，接着往上爬
                Some(Prod::ListTail) => cur = parent,
                _ => return false,
            }
        }
        false
    }

    /// 这个节点是不是「前面还有语句」的语句位。不可达诊断只在这种位置报：
    /// 一个块里的第一条语句永远可达（它所在的帧刚开），从第二条起才谈得上死代码。
    ///
    /// 两种形状：`stat_list stat` 那条唯一的两孩子 @ListTail 的末位，
    /// 以及 @Block 里跟在语句列表后面的 retstat
    fn follows_stat(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> bool {
        let Some(parent) = ast.get_node(node).parent else {
            return false;
        };
        let p = ast.get_node(parent);
        match self.get_production(ast, parent) {
            Some(Prod::ListTail) => p.children.len() == 2 && p.children[1] == node,
            Some(Prod::Block) => p.children.first() != Some(&node),
            _ => false,
        }
    }

    /// 这个 block 是不是某条语句的分支块。funcbody 的 block 不算 —— 它的帧在
    /// @FuncBody 的 enter 里就开好了（`flow_enter_body`，多带一个边界位），
    /// 这里再开一格就成了两层，`flow_leave_branch` 会把结果攒到错的那一帧
    fn block_is_branch(&self, ast: &Tree<NodeSyntax<'_>>, block: usize) -> bool {
        ast.get_node(block).parent.is_some_and(|p| {
            matches!(
                self.get_production(ast, p),
                Some(
                    Prod::Do
                        | Prod::While
                        | Prod::Repeat
                        | Prod::If
                        | Prod::IfElse
                        | Prod::ElseIf
                        | Prod::ForNum
                        | Prod::ForNumStep
                        | Prod::ForIn
                )
            ) as bool
        })
    }

    /// 这个列表节点是不是脊顶。左递归的上一节必然以本节点作累加器（children[0]），
    /// 元素永远在末位，所以「父节点是同族标签且 children[0] 是我」就还没到顶
    fn is_spine_top(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> bool {
        let Some(parent) = ast.get_node(node).parent else {
            return true;
        };
        match self.get_production(ast, parent) {
            Some(Prod::ListTail) => ast.get_node(parent).children[0] != node,
            _ => true,
        }
    }

    /// 摊平左递归列表脊，按源码顺序返回元素节点。
    /// `list : elem | list sep elem` 这一族形状一致 —— 累加器在 children[0]、元素在末位，
    /// 包括 classfields 在内的十一条都贴 @ListTail，所以这里只认一个标签
    fn spine(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = node;
        loop {
            let n = ast.get_node(cur);
            match n.get_data() {
                NodeSyntax::Prod(Some(Prod::ListTail)) => {
                    out.push(*n.children.last().unwrap());
                    cur = n.children[0];
                }
                // `fieldlist : fields fieldsep` / `classfieldlist : classfields ','`：
                // 无标签但两个符号，不满足折叠条件，且没有信息，要继续下探
                NodeSyntax::Prod(None)
                    if n.children.len() == 2
                        && matches!(
                            ast.get_node(n.children[1]).get_data(),
                            NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE(',' | ';')))
                        ) =>
                {
                    cur = n.children[0]
                }
                //剩下的其他None产生式，大概率到头了
                _ => {
                    out.push(cur);
                    break;
                }
            }
        }
        out.reverse();
        out
    }

    /// `decllist : NAME optype | decllist ',' NAME optype` 的脊，按源码顺序返回
    /// （NAME 节点, optype 节点）。元素是一**对**而不是单个节点，套不进 `spine()`
    fn decl_items(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut cur = node;
        loop {
            let n = ast.get_node(cur);
            match self.get_production(ast, cur) {
                Some(Prod::DeclRest) => {
                    out.push((n.children[2], n.children[3]));
                    cur = n.children[0];
                }
                // @DeclFirst：脊到底了
                _ => {
                    out.push((n.children[0], n.children[1]));
                    break;
                }
            }
        }
        out.reverse();
        out
    }

    /// 这条语句的各分支怎么合，`None` = 它不是带分支的语句。
    /// 驱动点拿它判「要不要 flow_open」，所以两边用同一张表，不会配不上。
    ///
    /// @ElseIf 不在表里：它的 block 是**外层 if** 的一支（左递归嵌套，
    /// 见 join_stack 的注释），自己不开帧也就不用合
    fn join_mode(prod: &Prod) -> Option<JoinMode> {
        Some(match prod {
            // 这两条盖住了所有路径
            Prod::IfElse => JoinMode::Exhaustive,
            // 没有 else 的 if、可能 0 次的循环、以及不知道会不会被调的函数体
            Prod::If
            | Prod::While
            | Prod::ForNum
            | Prod::ForNumStep
            | Prod::ForIn
            | Prod::FuncBody => JoinMode::Skippable,
            // 体至少跑一次
            Prod::Do | Prod::Repeat => JoinMode::Always,
            _ => return None,
        })
    }

    // ================== §3 名字表 ==================

    // ---- 类型名注册（走 scope_stack，支持块级作用域）----

    /// 在当前作用域注册类型名。同作用域内重名返回 false
    fn declare_type(&mut self, name: &str, r#ref: TypeRef) -> bool {
        let scope = self.scope_stack.last_mut().unwrap();
        if scope.type_names.contains_key(name) {
            false
        } else {
            scope.type_names.insert(name.to_string(), r#ref);
            true
        }
    }

    /// 从内向外查类型名，内层遮蔽外层；栈内全落空再问 Session 的内建名
    /// （array / table / 四个标量），所以用户自己声明的同名类型天然遮蔽内建
    fn lookup_type(&self, name: &str) -> Option<TypeRef> {
        for scope in self.scope_stack.iter().rev() {
            if let Some(r) = scope.type_names.get(name) {
                return Some(*r);
            }
        }
        self.session.lookup_builtin_type(name)
    }

    // ---- 变量注册（局部走 scope_stack，全局只在 Session）----

    /// 声明**局部**变量（`local` / 形参 / for 变量）：写进当前作用域。
    /// Lua 允许 `local x = 1; local x = 2` 在同一块里重新声明（后者遮蔽前者），
    /// 所以这里是覆盖语义，返回值只报告「是不是个新名字」；没有 local 的赋值
    /// 不是局部声明，走 declare_global。
    /// `assigned` = 声明语句本身就给了值（有初值 / 形参 / for 变量）
    fn declare_variable(
        &mut self,
        name: &str,
        ty: TypeId,
        span: Span,
        kind: VarScope,
        assigned: bool,
    ) -> bool {
        let slot = self.session.new_slot(ty, span, kind, assigned);
        let scope = self
            .scope_stack
            .last_mut()
            .expect("文件级作用域在 prepare 里已入栈");
        scope.variables.insert(name.to_string(), slot).is_none()
    }

    /// 登记全局变量（`G_config:Config = {...}`、`extern`、抬升上来的顶层函数名）。
    /// 全局只存 Session 这一份：复制进 scope_stack 就得两头同步，本文件里
    /// 后登记的全局立刻会和快照对不上
    fn declare_global(
        &mut self,
        name: &str,
        ty: TypeId,
        span: Span,
        kind: VarScope,
        assigned: bool,
    ) -> bool {
        let slot = self.session.new_slot(ty, span, kind, assigned);
        self.session.declare_global(name, slot)
    }

    /// 同层重复声明就报。Lua 允许同层遮蔽（`local x; local x`），TypeLua 不允许 ——
    /// 跨层遮蔽（内层 block 盖外层同名）不在此列，那查的是 last() 这一格。
    /// 报完照样让调用方接着 declare（覆盖旧格），免得后面每次读这个名字
    /// 都因为「查不到」再叠一条误报
    fn reject_same_scope_redecl(&self, name: &str, span: Span, log: &mut Vec<Logger>) {
        if self
            .scope_stack
            .last()
            .is_some_and(|s| s.variables.contains_key(name))
        {
            log.push(Logger {
                span,
                msg: format!("局部变量 {name} 在同一层作用域里重复声明"),
            });
        }
    }

    /// 赋值：把「已初始化」置上。从内向外找第一层持有这个名字的作用域就地置位，
    /// 栈内全落空再去 Session 兜全局。返回 false = 这名字压根没声明过。
    ///
    /// 只认**整体赋值**。`r.host = "x"` 里的 `r` 走的是读的那条路 ——
    /// 于是「不存在先声明再逐项注入字段」不必单独立规则，它自然报「r 可能尚未赋值」。
    ///
    /// 真把 false 推成 true 的那一次会记进 `init_record`：分支退出时靠它回滚。
    /// 对本来就 inited 的变量再赋值不留痕，所以日志里同一格最多出一次。
    ///
    /// 还兼着把这个名字的 nil 收窄抹掉：`if s ~= nil then s = nil end` 之后
    /// 那条收窄已经不成立了，留着就是不健全
    fn mark_inited(&mut self, name: &str) -> bool {
        let mut flipped: Option<InitFlip> = None;
        let mut found = false;
        for (depth, scope) in self.scope_stack.iter_mut().enumerate().rev() {
            scope.narrowed.remove(name);
            if let Some(slot) = scope.variables.get_mut(name) {
                if !slot.inited {
                    slot.inited = true;
                    flipped = Some(InitFlip {
                        name: name.to_string(),
                        scope: Some(depth),
                    });
                }
                found = true;
                break;
            }
        }
        if !found {
            match self.session.mark_global_inited(name) {
                None => return false,
                Some(true) => {
                    flipped = Some(InitFlip {
                        name: name.to_string(),
                        scope: None,
                    });
                }
                Some(false) => {}
            }
        }
        if let Some(mark) = flipped {
            self.init_record.push(mark);
        }
        true
    }

    /// 从内向外查变量，内层遮蔽外层；scope_stack 全落空才去 Session 兜全局，
    /// 「局部遮蔽全局」就是这个顺序的结果。
    ///
    /// 返回 `Option` 而不是 `TypeId`：「查不到」和「查到了但类型是 UNKNOWN」是
    /// 两回事，前者要报「未声明」，后者是上游推断失败，不该在它上面再叠一条误报
    fn lookup_variable(&self, name: &str) -> Option<VarInfo> {
        self.lookup_variable_at(name).map(|(_, slot)| slot)
    }

    /// 只问声明那一格，不带 nil 收窄。**写位**要的是这个：收窄说的是
    /// 「此刻的值更具体」，而声明才是「这个变量允许装什么」。
    /// `if s ~= nil then s = nil end` 里那次赋值是合法的 ——
    /// 它只是让收窄失效（`mark_inited` 顺手抹掉那一条）
    fn lookup_declared(&self, name: &str) -> Option<VarInfo> {
        for scope in self.scope_stack.iter().rev() {
            if let Some(slot) = scope.variables.get(name) {
                return Some(*slot);
            }
        }
        self.session.lookup_global(name)
    }

    /// 和 `lookup_variable` 同一条链，额外报出它落在哪一层：`Some(depth)` 是
    /// scope_stack 的下标，`None` 是 Session 里的全局。
    ///
    /// 「可能尚未赋值」要按变量归属决定查不查（见 `init_checkable`），光有类型不够。
    ///
    /// nil 收窄就插在这条链上：同一层先问 `variables` 再问 `narrowed`，
    /// 于是内层的收窄盖得住外层的声明，而本层真写了 `local s` 之后
    /// 那一格又盖回收窄 —— 正是 Lua 的语义次序。
    /// 收窄只换类型，`inited` / `decl_span` / 归属层次都跟原那格
    fn lookup_variable_at(&self, name: &str) -> Option<(Option<usize>, VarInfo)> {
        let mut narrowed: Option<TypeId> = None;
        for (depth, scope) in self.scope_stack.iter().enumerate().rev() {
            if let Some(slot) = scope.variables.get(name) {
                let mut slot = *slot;
                if let Some(ty) = narrowed {
                    slot.ty = ty;
                }
                return Some((Some(depth), slot));
            }
            // 里层的收窄优先：只记第一个碰上的
            if narrowed.is_none() {
                narrowed = scope.narrowed.get(name).copied();
            }
        }
        self.session.lookup_global(name).map(|slot| {
            let mut slot = slot;
            if let Some(ty) = narrowed {
                slot.ty = ty;
            }
            (None, slot)
        })
    }

    // ================== §5 分派 ==================
    //
    // 这两个函数是本文件的目录：`resolve` 分 Prod / Token 两路，
    // `resolve_prod` 一个标签一行地指向下面各节

    pub fn resolve(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) -> TypeId {
        let node = ast.get_node(node_index);
        let action = node.get_data();
        let typeid = match action {
            NodeSyntax::Prod(prod) => {
                if let Some(prod) = prod {
                    self.resolve_prod(ast, prod, node_index, log)
                } else {
                    //没有label的话，从子节点获取类型，从 buffer 推出 right.len 个子节点
                    let len = node.children.len();
                    if len == 1 {
                        return self.child_type(node.children[0]);
                    }
                    TypeId::UNKNOWN
                }
            }
            // 空产生式或无类型的 name 必然是叶节点
            NodeSyntax::Token(token) => {
                match token {
                    Token::STRING(_s) => TypeId::STRING,
                    Token::NUMERAL(_n) => TypeId::NUMBER,
                    // NAME 一律不查表。六条「无标签单孩子」的裸 NAME 产生式现在都
                    // 贴了标签（@VarRef / @TypeName / @TypeParam / @ImportItem / @FuncName），
                    // 所以能走到这里的 NAME 全是**名字位**：字段名、方法名、label、
                    // 形参名、class/typedef 名之类。它们的 parent 全是带标签的产生式且
                    // 下标固定，字符串由那些处理器自己去取；在这里查变量会把每个
                    // 字段名都当成一次变量读，一上线就是一屏误报
                    Token::NAME(_n) => TypeId::UNKNOWN,
                    // 类型位的 `nil`（`basictype : NIL`）和值位的 `nil`（`atom : NIL`）
                    // 折叠成同一个 token，而两处的类型都是 nil，所以不必贴标签区分
                    Token::RESERVED(Reserved::NIL) => TypeId::NIL,
                    Token::RESERVED(Reserved::TRUE | Reserved::FALSE) => TypeId::BOOLEAN,
                    Token::RESERVED(_r) => TypeId::UNKNOWN,
                    // 不带标注的 `...`：`vararg : ELLIPSIS` 单孩子被折叠掉，所以这个
                    // token 自己就是那一格 vararg。值位的 `...`（`atom : ELLIPSIS`）
                    // 是同一个形状、也是同一个意思 —— 零个或多个未经检查的值，
                    // 元素类型只能给 any。列表的截断 / 展开由 Pack 那套统一处理
                    Token::OPERATOR(OpType::ELLIPSIS) => {
                        self.session.type_arenas.pack(vec![], Some(TypeId::ANY))
                    }
                    Token::OPERATOR(_o) => TypeId::UNKNOWN,
                }
            }
        };
        //没有产生式只出的类型也登记
        self.register_value(node_index, typeid);
        typeid
    }

    fn resolve_prod(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        prod: &Prod,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) -> TypeId {
        match prod {
            // ---- 运算。参数是操作符 token 所在的孩子下标 ----
            Prod::BinOp => self.resolve_operator(ast, node_index, 1, log),
            Prod::UnOp => self.resolve_operator(ast, node_index, 0, log),

            // ARROW retspec
            // '(' multilist ')'
            // '(' explist ')'
            // '(' type ')'
            // ':' type
            // ELLIPSIS type
            Prod::ReturnType
            | Prod::RetMulti
            | Prod::Args
            | Prod::ParenType
            | Prod::TypeAnnotation => self.pass_through(ast, node_index, 1),
            // ELLIPSIS type —— 产出的是只有 vararg 槽的 Pack，不是裸元素类型：
            // `parlist : vararg` / `multilist : vararg` 都是单孩子折叠，这个节点
            // 会直接当参数表 / 返回列表用，形状得自带「变长」这一位
            Prod::VarargTyped => self.vararg_pack(ast, node_index),
            // NAME ':' type，名字纯文档
            // cast_exp AS type
            // NAME '=' exp
            Prod::ArgTypeNamed | Prod::Cast | Prod::FieldNamed => {
                self.pass_through(ast, node_index, 2)
            }
            Prod::FieldKV => self.pass_through(ast, node_index, 4), // '[' exp ']' '=' exp

            Prod::RetVoid | Prod::ArgsEmpty => TypeId::VOID,
            // 表达式位的 `{}` 和类型位的 `{}` 是同一个东西，intern 后同一个 TypeId
            Prod::TableEmpty | Prod::RecordEmpty => self.session.type_arenas.record(vec![]),
            // 非空的那两个同理：`'{' fieldlist '}'` 与 `'{' classfieldlist '}'`
            Prod::Table => self.resolve_fields(ast, node_index, FieldSite::Literal, log),
            Prod::Record => self.resolve_fields(ast, node_index, FieldSite::Record, log),

            // ---- 类型位 ----
            // `basictype : NAME` / `extendtype : NAME`：折叠前它和变量位的裸 NAME
            // 形状一样，靠标签才分得开该查 type_names 还是 variables
            Prod::TypeName => self.resolve_type_name(ast, node_index, log),
            Prod::FuncType => self.func_sig_type(ast, node_index),
            Prod::Union => self.resolve_union(ast, node_index),
            Prod::Intersect => self.resolve_intersect(ast, node_index),
            Prod::GenericType => self.resolve_generic_type(ast, node_index, log),
            // `X ',' vararg`：两处形状和语义都一样
            Prod::ParamsVararg | Prod::RetVararg => self.pack_vararg(ast, node_index),
            Prod::RetFixed => self.pack_fixed(ast, node_index),

            // ---- 表达式 ----
            // `prefixexp : NAME`，全 linter 唯一查变量的地方
            Prod::VarRef => self.resolve_var_ref(ast, node_index, log),
            Prod::Call => self.resolve_call(ast, node_index, log),
            Prod::MethodCall => self.resolve_method_call(ast, node_index, log),
            Prod::Paren => self.resolve_paren(ast, node_index),
            Prod::Index => self.resolve_index(ast, node_index, log),
            Prod::Dot => self.resolve_dot(ast, node_index, log),
            Prod::DottedName => self.resolve_dotted_name(ast, node_index, log),
            Prod::TurboFish => self.resolve_turbo_fish(ast, node_index, log),
            Prod::FuncExpr => self.resolve_func_expr(ast, node_index),
            Prod::FuncBody => self.func_sig_type(ast, node_index),

            // ---- 声明与绑定 ----
            Prod::Var => self.resolve_var(ast, node_index),
            Prod::Param => self.resolve_param(ast, node_index, log),
            Prod::MethodDef => self.resolve_method_def(ast, node_index, log),
            Prod::MethodSig => self.func_sig_type(ast, node_index),
            Prod::Generics => self.resolve_generics(ast, node_index, log),
            Prod::TypeParamBound => self.resolve_type_param_bound(ast, node_index, log),
            Prod::ListTail => self.resolve_list(ast, node_index, log),
            Prod::DeclFirst | Prod::DeclRest => self.resolve_decl_list(ast, node_index, log),

            // ---- 名字位：贴标签只为了不被当成变量读，名字由拥有者去取 ----
            // @TypeParam 归 @Generics（形参声明）、@ImportItem 归 @Import、
            // @FuncName 归 @FuncDecl / @MethodName（函数声明名的基名）。
            // @FuncName 不能复用 @VarRef：后序遍历下 @VarRef 的查表/诊断会在
            // @FuncDecl 登记名字之前跑，`function f() end` 会自己报自己未声明
            Prod::TypeParam | Prod::ImportItem | Prod::FuncName => TypeId::UNKNOWN,

            // ---- 语句：不产出类型，给 UNKNOWN（它**不是** VOID）----
            Prod::Import => {
                self.check_import(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::ImportAlias => {
                self.check_import_alias(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Pub => {
                self.check_pub(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Block => {
                self.check_block(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Do
            | Prod::While
            | Prod::Repeat
            | Prod::If
            | Prod::IfElse
            | Prod::ElseIf
            | Prod::ForNum
            | Prod::ForNumStep
            | Prod::ForIn => {
                self.check_control_flow(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Assign => {
                self.check_assign(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::ExprStat => {
                self.check_expr_stat(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Goto | Prod::Label => {
                self.check_jump(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Return | Prod::ReturnVoid => {
                self.check_return(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::LocalDecl => {
                self.check_local_decl(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::LocalDeclInit => {
                self.check_local_decl_init(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Extern => {
                self.check_extern(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::ClassDecl => {
                self.check_class_decl(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::ClassDeclExtends => {
                self.check_class_decl_extends(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::TypeDef => {
                self.check_type_def(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::FuncDecl => {
                self.check_func_decl(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::MethodName => self.resolve_method_name(ast, node_index),
            Prod::ClassBody => {
                self.check_class_body(ast, node_index, log);
                TypeId::UNKNOWN
            }
            Prod::FieldDecl => {
                self.check_field_decl(ast, node_index, log);
                TypeId::UNKNOWN
            }
        }
    }

    // ================== §12 占位 ==================
    //
    // 没有自己的类型、但在 §5 分派表里得占一行的产生式。它们不是「还没做」，
    // 而是「语义不在这个节点上」：该做的事由别处直接扮这颗子树完成，所以这里
    // 只需一个 UNKNOWN 把分派表填满。每条都写清了真正的处理点在哪

    /// `generics : '<' typeparams '>'` —— 声明一串泛型形参。铸 GenericId 并押进
    /// 作用域由 hoist_decl_generics / scope_func_generics 直接扮这个节点完成（走
    /// generic_param_names），形参表本身又由 generic_list_of 收进 `Types::Func`，
    /// 节点自己拿不出一个有意义的类型。标签仍要留：它是发射器的抹除入口之一
    fn resolve_generics(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) -> TypeId {
        TypeId::UNKNOWN
    }

    /// `decllist : NAME optype | decllist ',' NAME optype` —— 元素是 (NAME, optype)
    /// 一对、不是单个节点，所以套不进 spine()，也组不成一个类型。真正拆它的
    /// 是 check_local_decl_init：那里用 decl_items 逐对取名字与标注，配上 value_at
    /// 把右侧的 Pack 按位发下去
    fn resolve_decl_list(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) -> TypeId {
        TypeId::UNKNOWN
    }
}

impl Linter for TypeLinter {
    fn name(&self) -> &'static str {
        "type"
    }

    /// 换文件就重置局部状态；全局在 Session 里，不用管
    fn prepare(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str, _log: &mut Vec<Logger>) {
        // 当前模块名：pub 登记导出、hoist 登记 module_types 都要用，
        // 得赶在两趟 hoist 之前定好
        self.current_module = self.session.names.intern(module);
        // 节点下标是每棵树自己从 0 数的，不清就会让 register_value 报重入
        self.node_values.clear();
        self.scope_stack.clear();
        self.file_decls.clear();
        self.class_stack.clear();
        self.ret_stack.clear();
        // 文件级作用域：它是 flow_reset 记 scope_depth 的基准，得先压。
        // chunk 的 @Block 还会自己再压一格，顶层 local 落在那一格里
        self.scope_stack.push(Scope::new());
        self.flow_reset();
        // 变量名与类型名两条独立的抬升：函数名进 variables，class/typedef 名进
        // type_names。互不相干，先后无所谓
        self.hoist_top_level(ast);
        self.hoist_top_level_types(ast);
        // 名字占好了再把 class / typedef 的体提前填上：内联方法体里的 `self.x`
        // 要在主遍历里就查得到本类字段，而 P1 的 check_* 填体在各自 leave（方法体
        // 之后）才发生 —— 这一趟抢在主遍历之前把字段/父类/别名目标灌进 ClassInfo
        self.hoist_class_bodies(ast);
        // 最后把顶层全局函数的签名补给抬升过的名字：签名里引得到 class /
        // typedef，所以得等名字和体都填好（前三趟）之后才算
        self.hoist_func_signatures(ast);
    }

    /// 前序：作用域与流帧的「进」、以及两条非前序不可的诊断。
    ///
    /// 哪些事必须在这里而不能拉到 leave：
    ///   - 作用域/流帧得包住子树，开在后序就来不及了
    ///   - @VarRef 的两条诊断：看 resolve_var_ref 的注释
    ///   - `local function f` 得先声明名字才递归得了
    fn enter(&mut self, ast: &Tree<NodeSyntax<'_>>, node: usize, log: &mut Vec<Logger>) {
        // label 得在可达性检查之前处理：它是跳转的落点，前一条 `goto` / `return`
        // 终结不了它。降级也放在这里 —— 向前跳的那一段代码在 label 之前就走过了，
        // 但从 label 往后的 inited 结论全是那一跳污染出来的
        if self.get_production(ast, node) == Some(Prod::Label) {
            self.flow_untrust();
            self.flow_revive();
        }
        self.check_reachable(ast, node, log);
        let NodeSyntax::Prod(Some(prod)) = ast.get_node(node).get_data() else {
            return;
        };
        match prod {
            Prod::Block => {
                // 先开帧再压作用域：反了的话本块自己那格会被当成外层，
                // 内层遮蔽同名变量时回滚就打错人（见定值分析那节的注释）
                if self.block_is_branch(ast, node) {
                    self.flow_enter_branch();
                }
                self.scope_stack.push(Scope::new());
                self.bind_loop_vars(ast, node);
                // 收窄得在作用域压完之后：它就放在本块那一格里，
                // 出块一弹就失效
                self.bind_narrowing(ast, node);
            }
            // 帧开在这里而不是体的 block 上：这一格多带一个边界位（return 不终结
            // 外层、goto 的降级到此为止），而且形参要比帧晚一步进作用域
            Prod::FuncBody => {
                self.flow_open();
                self.flow_enter_body();
                // rettype 槽就在本节点上；名字取得到才有（函数表达式没名字）
                self.push_ret_frame(ast, node, node);
                // 泛型形参和形参共用这一格：两者的可见范围都是整个 funcbody，
                // 比体的 block 大一圈（rettype 也得看得见泛型形参）
                self.scope_stack.push(Scope::new());
                // 形参标注 / 返回类型 / 体里的 `T` 要解析成 Generic，泛型形参得先挂进这格。
                // 每进一次现铸现挂：funcbody 不是名义声明，没有 DeclId 存身份
                self.scope_func_generics(ast, node);
                // 类体外方法的 funcname 是前一个兄弟节点，此时已经后序求值完、接收者
                // TypeId 正挂在 node_values 上。':' 形式才注入隐式 self；'.' 形式写出的
                // self 由 resolve_param 经同一颗 funcname 节点取默认类型
                if let Some((ty, true)) = self.funcbody_receiver(ast, node) {
                    self.declare_variable("self", ty, ast.span_of(node), VarScope::Local, true);
                }
            }
            // 方法体和函数体同款：多带一个边界位（体内 return 不终结类体外的流），
            // 形参比体的 block 早一步进这一格 —— methodsig 在文法上先于 block
            Prod::MethodDef => {
                self.flow_open();
                self.flow_enter_body();
                // 方法的 rettype 槽在 methodsig（children[0]）上，不在体这个节点上
                let sig = ast.get_node(node).children[0];
                self.push_ret_frame(ast, node, sig);
                self.scope_stack.push(Scope::new());
            }
            // 进类体：把本类的 Ref 押上，methodsig 里不标注的 self 靠它取默认类型。
            // 再单开一格把泛型形参挂进去 —— 字段/方法签名/extendtype 里的 `T` 都在
            // 这棵子树内解析，得让它们查得到（P0a 已铸好身份，这里只按名字挂上）
            Prod::ClassDecl | Prod::ClassDeclExtends => {
                let ty = self.class_ref_of(ast, node);
                self.class_stack.push(ty);
                self.scope_stack.push(Scope::new());
                self.scope_decl_generics(ast, node);
            }
            // typedef 的目标类型里也能引用自己的泛型形参（`typedef Box<T> = {v:T}`），
            // 同样单开一格挂形参；没有 self、不进 class_stack
            Prod::TypeDef => {
                self.scope_stack.push(Scope::new());
                self.scope_decl_generics(ast, node);
            }
            Prod::VarRef => self.check_var_read(ast, node, log),
            // `local function f` = `local f; f = function...`，名字得在体之前就在，
            // 否则递归调用自己会报未声明。类型等 leave 里覆
            Prod::FuncDecl if ast.get_node(node).children.len() == 4 => {
                if let Some(name) = self.get_child_name(ast, node, 2) {
                    let span = ast.span_of(node);
                    // `local function f` 的名字也是局部变量，同层重名照拦。这里是它
                    // 唯一的声明点（leave 只补类型、不再 insert），所以不会自撞
                    self.reject_same_scope_redecl(name, span, log);
                    self.declare_variable(name, TypeId::UNKNOWN, span, VarScope::Local, true);
                }
            }
            // 带分支的语句：开一格 join 帧，各分支由它们自己的 block 去填
            p => {
                if Self::join_mode(p).is_some() {
                    self.flow_open();
                }
            }
        }
    }

    /// 后序：先算类型，再收作用域与流帧。
    ///
    /// 顺序不能反：`resolve` 里的声明动作（形参、local）得落在当前还没弹的
    /// 那格作用域里，而 `is_noreturn_stat` 更是非后序不可
    fn leave(&mut self, ast: &Tree<NodeSyntax<'_>>, node: usize, log: &mut Vec<Logger>) {
        self.resolve(ast, node, log);
        match ast.get_node(node).get_data() {
            NodeSyntax::Prod(Some(prod)) => match prod {
                Prod::Block => {
                    let repeat_body = ast
                        .get_node(node)
                        .parent
                        .is_some_and(|p| self.get_production(ast, p) == Some(Prod::Repeat));
                    if self.block_is_branch(ast, node) {
                        self.flow_leave_branch();
                    }
                    if repeat_body {
                        // `repeat local x = 1 until x == 1` 在 Lua 里合法：until 条件
                        // 看得见体内的局部。所以这格作用域留给 @Repeat 去弹，
                        // 而合并提到条件之前做（体必执行，本来就该先生效），
                        // 否则条件里读体内赋过的外层变量会误报「可能尚未赋值」
                        self.flow_close(JoinMode::Always);
                    } else {
                        self.scope_stack.pop();
                    }
                }
                // 方法体收口和函数体一模一样
                Prod::FuncBody | Prod::MethodDef => {
                    self.scope_stack.pop();
                    // 弹得比 resolve 晚：本节点的类型就是那个函数类型，没标注时
                    // 它的返回列表要用帧上攒的 return 推出来
                    self.pop_ret_frame(node);
                    self.flow_leave_branch();
                    self.flow_close(JoinMode::Skippable);
                }
                // 出类体：弹掉泛型形参那格作用域，再弹本类的 Ref
                Prod::ClassDecl | Prod::ClassDeclExtends => {
                    self.scope_stack.pop();
                    self.class_stack.pop();
                }
                // 出 typedef：弹掉泛型形参那格
                Prod::TypeDef => {
                    self.scope_stack.pop();
                }
                // 合并已经在体的 block 里做完了，这里只补那一次没弹的作用域
                Prod::Repeat => {
                    self.scope_stack.pop();
                }
                Prod::ExprStat => {
                    if self.is_noreturn_stat(ast, node) {
                        self.flow_terminate();
                    }
                }
                Prod::Return | Prod::ReturnVoid => self.flow_terminate(),
                // 向后跳让一遍前序合并失效，两头都降级：向后跳的 label 必先于 goto
                // 被看到，向前跳的 goto 必先于它跳过的代码，合起来盖得住
                Prod::Goto => {
                    self.flow_untrust();
                    self.flow_terminate();
                }
                p => {
                    if let Some(mode) = Self::join_mode(p) {
                        self.flow_close(mode);
                    }
                }
            },
            // `stat : BREAK` 是无标签单孩子，被折叠成了这个 token 节点本身。
            // BREAK 在整张文法里只出现在这一处，所以认 token 就够
            NodeSyntax::Token(Token::RESERVED(Reserved::BREAK)) => self.flow_terminate(),
            _ => {}
        }
    }

    /// 驱动点配对的自检。走完一棵树，栈上只该剩 flow_reset 压的文件帧和
    /// prepare 压的文件作用域。不平就是某条语句的 enter/leave 漏了一半，
    /// 而那种错不会当场爆，只会让后面的诊断莫名其妙地少一条多一条
    fn finish(&mut self, log: &mut Vec<Logger>) {
        // 字段覆盖的收口留到这里：extends 的父类可能声明在子类后面（前向继承），
        // 子类 leave 时父类体还没填，只有全走完才查得准
        self.check_field_overrides(log);
        debug_assert_eq!(self.flow_stack.len(), 1, "流帧栈没配平");
        debug_assert!(self.join_stack.is_empty(), "join 帧没配平");
        debug_assert!(self.ret_stack.is_empty(), "函数体栈没配平");
        debug_assert_eq!(self.scope_stack.len(), 1, "作用域栈没配平");
    }
}

/// 测试搬去了 type_linter/tests.rs：定值分析的内部单测和驱动点的端到端
/// 各占一节，正文这边只留这一行
#[cfg(test)]
mod tests;
