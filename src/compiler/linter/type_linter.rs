use super::types::*;
use super::*;
use crate::parser::ast::Tree;
use crate::parser::parser::*;
use crate::lexer::type_def::*;
use crate::parser::table::*;

// 不再有 Value：类型列表现在是一等的 `Types::Pack(ListId)`，和别的类型一样用 TypeId 携带。
// 装进去用 `type_arenas.pack(..)`，拆出来用 `type_arenas.as_list(..)`，中间只走 TypeId 一条道。

use core::panic;
use std::collections::HashMap;
struct Scope {
    variables: HashMap<String, TypeId>,
    type_names: HashMap<String, TypeRef>,  // 块级类型名：声明时注册，内层遮蔽外层
}

impl Scope {
    fn new() -> Self {
        Scope {
            variables: HashMap::new(),
            type_names: HashMap::new(),
        }
    }
}

pub struct TypeLinter<'a>{
    // 这里都是一些要传递继承属性而存在的缓冲区，最理想的状况是
    // 这里什么也没有，可以从子节点自然继承
    //todo node_index 本身是稠密的 arena 下标，Tree 暴露节点总数后这里能换成 Vec<TypeId>
    node_values:HashMap<usize, TypeId>,
    scope_stack: Vec<Scope>,
    ast: &'a Tree<NodeSyntax<'static>>,
    /// 旧的 File + Global 合并进 Session：名字/类型/声明/导出都在这一层。
    ///
    /// **自持,不外借**。共享的只是类型，这件事不该被 linter 层感知：`&mut Session`
    /// 得由搭 LintDriver 的那一层提供，会逼着通用 lint 层认识类型系统，还和
    /// `Box<dyn Linter>` 的 'static 打架、独占借用挡住 emitter。
    ///
    /// 跨文件累积靠的是**粒度**：TypeLinter 的粒度是「类型阶段」而非「一个文件」，
    /// 同一个实例被驱动着依次跑过各文件，Session 自然攒起来。per-file 的只有
    /// node_values / scope_stack，该在 prepare() 里重置
    session: Session,
}


impl<'a> TypeLinter<'a> {
    pub fn new(ast: &'a Tree<NodeSyntax<'static>>) -> Self {
        TypeLinter {
            node_values: HashMap::new(),
            scope_stack: Vec::new(),
            ast,
            session: Session::new(),
        }
    }
    
    fn register_value(&mut self, node_index: usize, ty: TypeId) {
        if self.node_values.contains_key(&node_index) {
            panic!("reentry node {} not allowed", node_index)
        }
        self.node_values.insert(node_index, ty);
    }

    /// 取子节点已注册的类型。没注册（标点符号、未实现的分支）算 UNKNOWN
    #[inline]
    fn child_type(&self, node_index: usize) -> TypeId {
        self.node_values.get(&node_index).copied().unwrap_or(TypeId::UNKNOWN)
    }

    /// 取第 `child` 个孩子的类型。`'(' x ')'`、`ARROW x`、`NAME ':' x` 这类
    /// 「标点包着一个真家伙」的产生式全是这个形状
    #[inline]
    fn pass_through(&self, node_index: usize, child: usize) -> TypeId {
        self.child_type(self.ast.get_node(node_index).children[child])
    }

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
    /// 没有 local 的赋值不是局部声明，走 declare_global
    fn declare_variable(&mut self, name: &str, ty: TypeId) -> bool {
        let scope = self.scope_stack.last_mut().expect("文件级作用域在 prepare 里已入栈");
        scope.variables.insert(name.to_string(), ty).is_none()
    }

    /// 登记全局变量（`G_config:Config = {...}`，以及赋值给一个查不到的名字）。
    /// 全局只存 Session 这一份：复制进 scope_stack 就得两头同步，本文件里
    /// 后登记的全局立刻会和快照对不上
    fn declare_global(&mut self, name: &str, ty: TypeId) -> bool {
        self.session.declare_global(name, ty)
    }

    /// 从内向外查变量，内层遮蔽外层；scope_stack 全落空才去 Session 兜全局，
    /// 「局部遮蔽全局」就是这个顺序的结果。
    /// 兜到底是 UNKNOWN：读没赋值过的全局在 Lua 里合法（值是 nil）
    fn lookup_variable(&self, name: &str) -> TypeId {
        self.scope_stack
            .iter()
            .rev()
            .find_map(|scope| scope.variables.get(name).copied())
            .or_else(|| self.session.lookup_global(name))
            .unwrap_or(TypeId::UNKNOWN)
    }

        /// 多值截成单值：非末位的多值表达式、以及 `(f())` 都只留第一个值。
    /// 一个值都不产出（`-> ()`）时是 nil —— Lua 里少给的实参就是 nil
    fn truncate_ret(&self, ty: TypeId) -> TypeId {
        match self.session.type_arenas.get_type(ty) {
            Types::Pack(l) => {
                let list = self.session.type_arenas.list(l);
                // 只有 vararg 的 Pack（`-> ...T`）截出来是它的元素类型
                list.fixed.first().copied().or(list.vararg).unwrap_or(TypeId::NIL)
            }
            _ => ty,
        }
    }

    fn resolve_prod(&mut self, prod:&Prod, node_index:usize, log:&mut Vec<Logger>)->TypeId{
        match prod {
            // ---- 运算。参数是操作符 token 所在的孩子下标 ----
            Prod::BinOp => self.resolve_operator(node_index, 1),
            Prod::UnOp => self.resolve_operator(node_index, 0),

            // ARROW retspec
            // '(' multilist ')'
            // '(' explist ')'
            // '(' type ')'
            // ':' type
            // ELLIPSIS type
            Prod::ReturnType | Prod::RetMulti | Prod::Args |
            Prod::ParenType | Prod::TypeAnnotation | Prod::VarargTyped=> self.pass_through(node_index, 1),      
            // NAME ':' type，名字纯文档
            // cast_exp AS type
            // NAME '=' exp
            Prod::ArgTypeNamed |Prod::Cast |Prod::FieldNamed => self.pass_through(node_index, 2),    
            Prod::FieldKV => self.pass_through(node_index, 4),         // '[' exp ']' '=' exp

            
            Prod::RetVoid | Prod::ArgsEmpty => TypeId::VOID,
            // 表达式位的 `{}` 和类型位的 `{}` 是同一个东西，intern 后同一个 TypeId
            Prod::TableEmpty | Prod::RecordEmpty => self.session.type_arenas.record(vec![]),
            // 非空的那两个同理：`'{' fieldlist '}'` 与 `'{' classfieldlist '}'`
            Prod::Table | Prod::Record => self.resolve_fields(node_index),

            // ---- 类型位 ----
            Prod::FuncType => self.resolve_func_type(node_index),
            Prod::Union => self.resolve_union(node_index),
            Prod::Intersect => self.resolve_intersect(node_index),
            Prod::GenericType => self.resolve_generic_type(node_index),
            // `X ',' vararg`：两处形状和语义都一样
            Prod::ParamsVararg | Prod::RetVararg => self.pack_vararg(node_index),
            Prod::RetFixed => self.pack_fixed(node_index),

            // ---- 表达式 ----
            Prod::Call => { let f = self.pass_through(node_index, 0); self.ret_of(f) }
            Prod::MethodCall => self.resolve_method_call(node_index),
            Prod::Paren => self.resolve_paren(node_index, log),
            Prod::Index => self.resolve_index(node_index, log),
            Prod::Dot => self.resolve_dot(node_index, log),
            Prod::DottedName => self.resolve_dotted_name(node_index, log),
            Prod::TurboFish => self.resolve_turbo_fish(node_index, log),
            Prod::FuncExpr => self.resolve_func_expr(node_index, log),
            Prod::FuncBody => self.resolve_func_body(node_index, log),

            // ---- 声明与绑定 ----
            Prod::Var => self.resolve_var(node_index, log),
            Prod::Param => self.resolve_param(node_index, log),
            Prod::MethodDecl => self.resolve_method_decl(node_index, log),
            Prod::MethodDef => self.resolve_method_def(node_index, log),
            Prod::MethodSig => self.resolve_method_sig(node_index, log),
            Prod::Generics => self.resolve_generics(node_index, log),
            Prod::TypeParamBound => self.resolve_type_param_bound(node_index, log),
            Prod::ListTail => self.resolve_list(node_index, log),
            Prod::DeclFirst | Prod::DeclRest => self.resolve_decl_list(node_index, log),

            // ---- 语句：不产出类型，给 UNKNOWN（它**不是** VOID）----
            Prod::Import => { self.resolve_import(node_index, log); TypeId::UNKNOWN }
            Prod::ImportAlias => { self.resolve_import_alias(node_index, log); TypeId::UNKNOWN }
            Prod::Pub => { self.resolve_pub(node_index, log); TypeId::UNKNOWN }
            Prod::Block => { self.check_block(node_index, log); TypeId::UNKNOWN }
            Prod::Do | Prod::While | Prod::Repeat | Prod::If | Prod::IfElse
            | Prod::ElseIf | Prod::ForNum | Prod::ForNumStep | Prod::ForIn => {
                self.check_control_flow(node_index, log);
                TypeId::UNKNOWN
            }
            Prod::Assign => { self.check_assign(node_index, log); TypeId::UNKNOWN }
            Prod::ExprStat => { self.check_expr_stat(node_index, log); TypeId::UNKNOWN }
            Prod::Goto | Prod::Label => { self.check_jump(node_index, log); TypeId::UNKNOWN }
            Prod::Return | Prod::ReturnVoid => { self.check_return(node_index, log); TypeId::UNKNOWN }
            Prod::LocalDecl => { self.check_local_decl(node_index, log); TypeId::UNKNOWN }
            Prod::LocalDeclInit => { self.check_local_decl_init(node_index, log); TypeId::UNKNOWN }
            Prod::ClassDecl => { self.check_class_decl(node_index, log); TypeId::UNKNOWN }
            Prod::ClassDeclExtends => { self.check_class_decl_extends(node_index, log); TypeId::UNKNOWN }
            Prod::TypeDef => { self.check_type_def(node_index, log); TypeId::UNKNOWN }
            Prod::FuncDecl => { self.check_func_decl(node_index, log); TypeId::UNKNOWN }
            Prod::MethodName => { self.resolve_method_name(node_index, log); TypeId::UNKNOWN }
            Prod::ClassBody => { self.resolve_class_body(node_index, log); TypeId::UNKNOWN }
            Prod::FieldDecl => { self.resolve_field_decl(node_index, log); TypeId::UNKNOWN }
        }
    }

    // 表达式运算
    /// `op_child` 是操作符 token 所在的下标：BinOp 在中间（1）、UnOp 在最前（0）。
    /// 一元和二元的 token 集不相交（`and`/`or`/`..` 不可能一元，`not`/`#` 不可能二元），
    /// 所以两张表并成一张，prod 就不用传了
    fn resolve_operator(&mut self, node_index: usize, op_child: usize) -> TypeId {
        let op = self.ast.get_node(node_index).children[op_child];
        let NodeSyntax::Token(op_token) = self.ast.get_node(op).get_data() else {
            return TypeId::UNKNOWN;
        };
        match op_token {
            Token::RESERVED(Reserved::AND | Reserved::OR | Reserved::NOT) => TypeId::BOOLEAN,
            Token::OPERATOR(OpType::EQ | OpType::NE | OpType::LE | OpType::GE) => TypeId::BOOLEAN,
            Token::OPERATOR(OpType::SIMPLE('<' | '>')) => TypeId::BOOLEAN,
            Token::OPERATOR(
                OpType::SIMPLE('+' | '-' | '*' | '/' | '%' | '^' | '#' | '~') | OpType::IDIV,
            ) => TypeId::NUMBER,
            Token::OPERATOR(OpType::SHL | OpType::SHR) => TypeId::NUMBER,
            Token::OPERATOR(OpType::CONCAT) => TypeId::STRING,
            _ => TypeId::UNKNOWN,
        }
    }

    //todo Lua 的括号会把多值截成一个值（`(f())` 只留第一个），所以不能简单
    //passthru —— 改成 `truncate_ret(passthru(node_index, 1))` 即可
    fn resolve_paren(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }

    // 访问与调用
    /// `prefixexp '[' exp ']'` —— 只有内建容器能静态定出元素类型。
    /// record 没有字面量类型（算不出键到底是哪个字段）、class 字段该走 `.`，
    /// 所以它们和 any 一样给 ANY：渐进类型的逃逸口，不是 UNKNOWN
    fn resolve_index(&mut self, node_index: usize, log: &mut Vec<Logger>) -> TypeId {
        let base = self.pass_through(node_index, 0);
        // `t[f()]`：键位不是末位，多值要截到一个
        let key = self.truncate_ret(self.pass_through(node_index, 2));
        let span = self.ast.span_of(node_index);
        if let Some(elem) = self.session.type_arenas.as_array(base) {
            self.expect_key(key, TypeId::NUMBER, span, log, "数组下标");
            return elem;
        }
        if let Some((k, v)) = self.session.type_arenas.as_map(base) {
            self.expect_key(key, k, span, log, "表键");
            return v;
        }
        // 剩下的里面有确定不是表的：索引 nil / number / boolean 在 Lua 里是运行时错误
        // （string 不拦 —— 它有元表，`s[1]` 只是 nil 而不报错）
        if matches!(base, TypeId::NIL | TypeId::NUMBER | TypeId::BOOLEAN) {
            log.push(Logger {
                span,
                msg: format!("不能索引 {} 类型的值", self.session.show(base)),
            });
        }
        TypeId::ANY
    }

    /// 键类型的粗检。真正的可赋值判定（union / 名义子类型 / 宽度子类型）要等
    /// assignable，这里只拦「写错得很明白」的：any / unknown / 泛型形参一律放过，
    /// 否则在类型信息还不全的阶段会刷一屏误报
    fn expect_key(&self, got: TypeId, want: TypeId, span: Span, log: &mut Vec<Logger>, what: &str) {
        if got == want || matches!(got, TypeId::ANY | TypeId::UNKNOWN) {
            return;
        }
        if matches!(self.session.type_arenas.get_type(got), Types::Generic(_)) {
            return;
        }
        log.push(Logger {
            span,
            msg: format!(
                "{}应为 {}，实际是 {}",
                what,
                self.session.show(want),
                self.session.show(got)
            ),
        });
    }

    fn resolve_dot(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    fn resolve_dotted_name(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    /// 在 owner 上查名为 name 的字段/方法，返回**已代入实参**的类型。
    /// 字段类型存的是类自己的泛型形参，所以查到就得 subst。
    /// 沿 extends 链上溯时，父类的 Ref 实参先用子类的替换过一遍 ——
    /// `class Flipped<A,B> : Pair<B,A>` 的重排靠这一步才不丢。
    /// resolve_dot / resolve_index / resolve_method_call 也都该走它
    fn lookup_field(&mut self, owner: TypeId, name: NameId) -> Option<TypeId> {
        let mut cur = owner;
        // extends 成环该由 check_class_decl_extends 报错；这里只负责不挂死
        for _ in 0..64 {
            match self.session.type_arenas.get_type(cur) {
                // record 是普通table，没有元表
                Types::Record(f) => {
                    return self
                        .session
                        .type_arenas
                        .fields(f)
                        .iter()
                        .find(|fld| fld.name == name)
                        .map(|fld| fld.ty);
                }
                Types::Ref { decl, args } => {
                    let generics = self.session.decls.get(decl).generics.clone();
                    let arg_tys = self.session.type_arenas.list(args).fixed.clone();
                    let s = self.session.type_arenas.intern_subst(&generics, &arg_tys);
                    // 先把要用的拷成 Copy 值，借用结束后再动 arena
                    let (hit, next) = match &self.session.decls.get(decl).body {
                        DeclBody::Class(c) => (
                            c.fields
                                .iter()
                                .find(|f| f.field.name == name)
                                .map(|f| f.field.ty),
                            c.extends,
                        ),
                        // 别名：展开定义体再查。递归 typedef 靠上面的圈数兜底
                        DeclBody::Typedef(t) => (None, Some(t.target)),
                    };
                    match hit {
                        Some(ty) => return Some(self.session.type_arenas.subst(ty, s)),
                        None => cur = self.session.type_arenas.subst(next?, s),
                    }
                }
                // 交类型：成员里依次查，命中即返。record & record 已在 intersect()
                // 里合并，所以这里面最多一个 record，剩下是 Ref/table 这类不可约成员
                Types::Intersect(l) => {
                    let members = self.session.type_arenas.list(l).fixed.clone();
                    return members.iter().find_map(|&m| self.lookup_field(m, name));
                }
                _ => return None,
            }
        }
        None
    }

    /// 调用一个可调用类型得到的值。ret 是列表，单个会被 pack_of 折回裸类型
    fn ret_of(&mut self, callee: TypeId) -> TypeId {
        match self.session.type_arenas.get_type(callee) {
            Types::Func { ret, .. } => self.session.type_arenas.pack_of(ret),
            _ => TypeId::UNKNOWN,
        }
    }

    //todo 调用产出多值时返回 Pack。注意 Lua 的截断/展开规则：只有末位的多值
    //表达式才展开，非末位要截到 1 个值——Pack 不自动扁平化，这一步必须手写
    /// prefixexp ':' NAME args
    fn resolve_method_call(&mut self, node_index: usize) -> TypeId {
        let children = &self.ast.get_node(node_index).children;
        //prefix是class/table/array(后两者都是class)没跑了
        let recv = self.child_type(children[0]);
        let name = match self.ast.get_node(children[2]).get_data() {
            NodeSyntax::Token(Token::NAME(n)) => self.session.names.intern(n),
            _ => panic!("method name must be identifier"),
        };
        // 查不到先算 UNKNOWN：“没这个方法”归后面的检查阶段报，传递阶段不发诊断
        match self.lookup_field(recv, name) {
            Some(m) => self.ret_of(m),
            None => TypeId::UNKNOWN,
        }
    }
    fn resolve_turbo_fish(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }

    // 表构造
    /// `'{' fieldlist '}'`（表字面量）与 `'{' classfieldlist '}'`（类型位的匿名 record）。
    /// 两边走的是不同的列表非终结符，但形状和终点一样 —— 摊脊取元素、逐个收成
    /// 字段、intern 成 record。区别全在「元素怎么变成 Field」，交给 collect_field
    fn resolve_fields(&mut self, node_index: usize) -> TypeId {
        let list = self.ast.get_node(node_index).children[1];
        let mut fields = Vec::new();
        for item in self.spine(list) {
            if let Some(f) = self.collect_field(item) {
                fields.push(f);
            }else {
                let spine_node = self.ast.get_node(item);
            }
        }
        self.session.type_arenas.record(fields)
    }

    /// 列表的一项 -> record 字段。进不了 record 的返回 None。
    ///
    /// 按**元素**标签 dispatch 而不是按父产生式，因为两侧的元素标签集不相交：
    /// fields 只出 @FieldNamed / @FieldKV / 裸 exp，classfields 只出 @FieldDecl /
    /// @MethodDecl / @MethodDef。`NAME '=' exp` 两边都能写，但表达式位归约成
    /// @FieldNamed、类型位归约成 @FieldDecl，文法已经把它们分开了，所以这个
    /// match 没有死分支，也不会让类型位放进本该被文法拦掉的形式
    fn collect_field(&mut self, node: usize) -> Option<Field> {
        match self.ast.get_node(node).get_data() {
            // NAME '=' exp：类型从 exp 推（@FieldNamed 已经把 children[2] 透上来）
            NodeSyntax::Prod(Some(Prod::FieldNamed)) => {
                let name_node = self.ast.get_node(node).children[0];
                let ty = self.child_type(node);
                let NodeSyntax::Token(Token::NAME(n)) = self.ast.get_node(name_node).get_data()
                else {
                    return None;
                };
                let name = self.session.names.intern(n);
                // default 说的是**声明**有没有默认值，字面量里没这个概念。必须填 false：
                // 类型位的 record 一律 false（`= exp` 由 checker 拒绝），填 true 会让
                // `local t: {a:number} = {a=1}` 两边 intern 成不同 TypeId
                Some(Field { name, ty, default: false })
            }
            NodeSyntax::Prod(Some(Prod::FieldDecl)) => Some(self.collect_field_decl(node)),
            NodeSyntax::Prod(Some(Prod::FieldKV)) => {
                let children = self.ast.get_node(node).children[1];
                let node = self.ast.get_node(children);
                match node.get_data() {
                    NodeSyntax::Token(Token::STRING(s)) => {
                        // s strip掉前后引号
                        let s_content = &s[1..s.len() - 1];
                        Some(Field {
                        name: self.session.names.intern(s_content),
                        ty: self.child_type(children),
                        default: false,
                    })},
                    //动态字段：键不是字面量（`[k] = v`），没有静态名字就成不了具名
                    // Field，所以不给 record 贡献字段、直接跳过。检测是上层的事
                    _ => None,
                }
            }
            NodeSyntax::Prod(Some(Prod::MethodDecl | Prod::MethodDef)) => {
                let sig = self.ast.get_node(node).children[0];
                let name_node = self.ast.get_node(sig).children[0];
                let NodeSyntax::Token(Token::NAME(n)) = self.ast.get_node(name_node).get_data()
                else {
                    return None;
                };
                let name = self.session.names.intern(n);
                let ty = self.pass_through(node, 0);
                Some(Field { name, ty, default: false })
            }
            _ => None,
        }
    }

    /// 摊平左递归列表脊，按源码顺序返回元素节点。
    /// `list : elem | list sep elem` 这一族形状一致 —— 累加器在 children[0]、元素在末位，
    /// 包括 classfields 在内的十一条都贴 @ListTail，所以这里只认一个标签
    fn spine(&self, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = node;
        loop {
            let n = self.ast.get_node(cur);
            match n.get_data() {
                NodeSyntax::Prod(Some(Prod::ListTail)) => {
                    out.push(*n.children.last().unwrap());
                    cur = n.children[0];
                }
                // `fieldlist : fields fieldsep` / `classfieldlist : classfields ','`：
                // 无标签但两个符号，不满足折叠条件，且没有信息，要继续下探
                NodeSyntax::Prod(None)
                    if n.children.len() == 2 && matches!(
                            self.ast.get_node(n.children[1]).get_data(),
                            NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE(',' | ';')))
                        ) => {
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

    // 函数相关
    fn resolve_func_expr(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    fn resolve_func_body(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    fn resolve_method_decl(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    fn resolve_method_def(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    fn resolve_method_name(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn resolve_method_sig(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }

    // 类型位
    /// functype : function '(' [argtypes] ')' [rettype]
    fn resolve_func_type(&mut self, node_index: usize) -> TypeId {
        // children 取所有权：循环里要用 &mut self.session，不能一直挂着 self.ast 的借用
        let children = self.ast.get_node(node_index).children.clone();
        // @ReturnType 折出来的子节点是返回列表，其余（argtypes 折出）当参数列表。
        // 单个类型会被 parser 折叠成裸类型、多个才是 Pack —— as_list 统一成 ListId。
        // 完整的参数/返回收集依赖 ListTail / retspec（仍是 stub），这里先按子节点
        // 已注册的类型组装
        let mut params = ListId::EMPTY;
        let mut ret = ListId::EMPTY;
        for &c in &children {
            let is_ret = matches!(
                self.ast.get_node(c).get_data(),
                NodeSyntax::Prod(Some(Prod::ReturnType))
            );
            // 未注册的子节点（FUNCTION / 括号）跳过
            if !self.node_values.contains_key(&c) {
                continue;
            }
            let ty = self.child_type(c);
            let list = self.session.type_arenas.as_list(ty);
            if is_ret { ret = list; } else { params = list; }
        }
        self.session.type_arenas.func(params, ret)
    }

    /// uniontype : uniontype '|' basictype —— union() 自带扁平化/排序/去重，
    /// 左边是不是 Union 都无所谓，收齐两侧类型丢进去即可
    fn resolve_union(&mut self, node_index: usize) -> TypeId {
        let left = self.pass_through(node_index, 0);
        let right = self.pass_through(node_index, 2);
        self.session.type_arenas.union(vec![left, right])
    }

    /// intertype : intertype '&' basictype —— 和 union 同型，intersect() 自带扁平化/
    /// 急合并/去重，两侧收齐丢进去即可。矛盾交（`number & string`）不在这报，
    /// 等 assignable / 声明处用 intersection_conflict 去查
    fn resolve_intersect(&mut self, node_index: usize) -> TypeId {
        let left = self.pass_through(node_index, 0);
        let right = self.pass_through(node_index, 2);
        self.session.type_arenas.intersect(vec![left, right])
    }

    /// NAME '<' typeargs '>'
    fn resolve_generic_type(&mut self, node_index: usize) -> TypeId {
        let children = self.ast.get_node(node_index).children.clone();
        let name = match self.ast.get_node(children[0]).get_data() {
            NodeSyntax::Token(Token::NAME(name)) => name.to_string(),
            _ => panic!("generic type must be identifier"),
        };
        // 单实参折叠成裸类型、多实参是 Pack，as_list 都能收成 ListId
        let arg_ty = self.child_type(children[2]);
        let args = self.session.type_arenas.as_list(arg_ty);
        // class 和 typedef 现在共用 TypeRef::Decl，是哪种由 DeclTable 说了算；
        // 类型值只记 Ref{decl, args}，真正的实例化（subst）推迟到查字段时才做
        let decl = match self.lookup_type(&name) {
            Some(TypeRef::Decl(id)) => id,
            Some(_) => panic!("{} is not a class or typedef", name),
            None => panic!("no class or typedef for generic type {}", name),
        };
        let ng = self.session.decls.get(decl).generics.len();
        let na = self.session.type_arenas.list(args).len();
        if ng != na {
            panic!("{} requires {} type args, got {}", name, ng, na);
        }
        self.session.type_arenas.reference(decl, args)
    }

    /// `typelist ',' type` —— 末位类型追加到定长部分。multilist 不左递归，
    /// 所以它自己一层就够，不必走 resolve_list 那套脊顶收集
    fn pack_fixed(&mut self, node_index: usize) -> TypeId {
        let mut fixed = self.fixed_of(node_index);
        fixed.push(self.pass_through(node_index, 2));
        self.session.type_arenas.pack(fixed, None)
    }

    /// `list ',' vararg` —— 末位是变长，进 vararg 槽。
    /// vararg 非 None，pack_of 的「长度 1 就折叠」不会触发，Pack 保得住
    fn pack_vararg(&mut self, node_index: usize) -> TypeId {
        let fixed = self.fixed_of(node_index);
        let elem = self.pass_through(node_index, 2);
        self.session.type_arenas.pack(fixed, Some(elem))
    }

    /// 取 children[0] 那个列表的定长部分。单个类型会被折成裸类型，as_list 统一收口。
    /// clone 是必须的：list() 借着 session，而 pack() 要 &mut
    fn fixed_of(&mut self, node_index: usize) -> Vec<TypeId> {
        let head = self.pass_through(node_index, 0);
        let l = self.session.type_arenas.as_list(head);
        self.session.type_arenas.list(l).fixed.clone()
    }

    /// '{' classfieldlist '}' 的字段收集已并入 resolve_fields，这里只管单条 @FieldDecl。
    /// FieldDecl: NAME ':' type  |  NAME ':' type '=' exp  |  NAME '=' exp
    fn collect_field_decl(&mut self, node_index: usize) -> Field {
        let children = self.ast.get_node(node_index).children.clone();
        let name = match self.ast.get_node(children[0]).get_data() {
            NodeSyntax::Token(Token::NAME(n)) => self.session.names.intern(n),
            _ => self.session.names.intern(""),
        };
        // 三条产生式共用 @FieldDecl，孩子数分不开（第一、三条都是 3 个），
        // 得看 children[1] 是 ':' 还是 '='：
        //   NAME ':' type           无默认值
        //   NAME ':' type '=' exp   有默认值
        //   NAME '=' exp            有默认值，类型从 exp 推
        let annotated = matches!(
            self.ast.get_node(children[1]).get_data(),
            NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE(':')))
        );
        // children[2] 三条都对：前两条是 type，第三条是 exp
        let ty = self.child_type(children[2]);
        let default = !annotated || children.len() > 3;
        Field { name, ty, default }
    }

    fn resolve_generics(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    //todo 形参上界：typeparam : NAME ':' type。解出 children[2] 的 TypeId 后
    //往 GenericInfo.constrain 里填。注意约束位是单个 type（union 算一个），
    //所以 constrain 应该是 Option<TypeId> 而不是 Option<Vec<TypeId>>
    fn resolve_type_param_bound(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }

    // 变量与绑定
    /// var : prefixexp optype
    fn resolve_var(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    /// param : NAME optype
    fn resolve_param(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }
    /// 这个列表节点是不是脊顶。左递归的上一节必然以本节点作累加器（children[0]），
    /// 元素永远在末位，所以「父节点是同族标签且 children[0] 是我」就还没到顶
    fn is_spine_top(&self, node: usize) -> bool {
        let Some(parent) = self.ast.get_node(node).parent else {
            return true;
        };
        let p = self.ast.get_node(parent);
        match p.get_data() {
            NodeSyntax::Prod(Some(Prod::ListTail)) => p.children[0] != node,
            _ => true,
        }
    }

    /// 列表脊 -> Pack。`list : elem | list sep elem` 这一族共用。
    ///
    /// **只在脊顶建一次**：脊上逐节点建表是 O(n²)（第 k 节点要把前 k 个元素
    /// 重新拷一遍并 intern 一张只用一次的中间列表），认出非脊顶直接返回 UNKNOWN、
    /// 顶上用 spine() 一次收齐就回到 O(n)。中间节点的值没人读 —— 消费点
    /// （@Args / @Assign / @RetFixed / @ParamsVararg …）拿到的都是脊顶那个。
    ///
    /// 多值的截断/展开也落在这里而不是各个消费点：只有脊顶同时知道「全部元素」
    /// 和「谁在末位」，`f(g(), h())` 里 g 截成 1 个值、h 原样展开这条规则
    /// 换到消费点就得每处重新推一遍。元素是类型（typeargs / typelist / argtypelist）
    /// 时两者都是恒等变换，不必分叉
    fn resolve_list(&mut self, node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        // `stat_list stat` 是十一条 @ListTail 里唯一没有分隔符的（len == 2）：串的是
        // 语句、不产出类型，形状本身就分得开，不用为它单开一个标签
        if self.ast.get_node(node_index).children.len() == 2 {
            return TypeId::UNKNOWN;
        }
        // 只有左递归顶才建list
        if !self.is_spine_top(node_index) {
            return TypeId::UNKNOWN;
        }
        let items = self.spine(node_index);
        // spine() 至少产出一个元素，即tail就是本list的节点
        let Some((&tail, head)) = items.split_last() else {
            return TypeId::UNKNOWN;
        };
        let mut fixed = Vec::with_capacity(items.len());
        for &item in head {
            let ty = self.child_type(item);
            fixed.push(self.truncate_ret(ty));
        }
        let mut vararg = None;
        let last = self.child_type(tail);
        match self.session.type_arenas.get_type(last) {
            // 末位的多值原样铺开：定长接到后面，vararg 继续当 vararg
            Types::Pack(l) => {
                let list = self.session.type_arenas.list(l);
                fixed.extend_from_slice(&list.fixed);
                vararg = list.vararg;
            }
            _ => fixed.push(last),
        }
        self.session.type_arenas.pack(fixed, vararg)
    }
    /// decllist : NAME optype | decllist ',' NAME optype —— 元素是 (NAME, optype) 一对，
    /// 不是单个节点，所以套不进 spine()
    fn resolve_decl_list(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) -> TypeId {
        TypeId::UNKNOWN
    }

    // 语句检查
    fn resolve_import(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn resolve_import_alias(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn resolve_pub(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_block(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_control_flow(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_assign(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_expr_stat(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_jump(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_return(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_local_decl(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_local_decl_init(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_class_decl(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_class_decl_extends(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_type_def(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn check_func_decl(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn resolve_class_body(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }
    fn resolve_field_decl(&mut self,  _node_index: usize, _log: &mut Vec<Logger>) {
    }

    pub fn resolve(&mut self,node_index:usize, log:&mut Vec<Logger>)->TypeId{
        let node = self.ast.get_node(node_index);
        let action = node.get_data();
        let typeid= match action {
            NodeSyntax::Prod(prod) => {
                if let Some(prod) = prod {
                    self.resolve_prod(prod, node_index, log)
                } else {
                    //没有label的话，从子节点获取类型，从 buffer 推出 right.len 个子节点
                    let len = node.children.len();
                    if len == 1 {
                        return self.child_type(node.children[0]);
                    }
                    TypeId::UNKNOWN
                }
            },
            // 空产生式或无类型的name必然是叶节点
            NodeSyntax::Token(token) => {
                match token {
                    Token::STRING(_s) => TypeId::STRING,
                    Token::NUMERAL(_n) => TypeId::NUMBER,
                    Token::NAME(n) => {
                        //查询一下本地变量
                        self.lookup_variable(n)
                    },
                    Token::RESERVED(_r) => TypeId::UNKNOWN,
                    Token::OPERATOR(_o) => TypeId::UNKNOWN,
                }
            },
        };
        //没有产生式只出的类型也登记
        self.register_value(node_index, typeid);
        typeid  
    }
}


impl<'a> Linter for TypeLinter<'a> {
    fn name(&self) -> &'static str {
        "type"
    }

    /// 换文件就重置局部状态；全局在 Session 里，不用管
    fn prepare(&mut self, _ast: &Tree<NodeSyntax<'_>>, _log: &mut Vec<Logger>) {
        
    }

    fn enter(&mut self, ast: &Tree<NodeSyntax<'_>>, node:usize, log:&mut Vec<Logger>){

    }

    fn leave(&mut self, ast: &Tree<NodeSyntax<'_>>, node:usize, log:&mut Vec<Logger>){

    }

    fn finish(&mut self, log:&mut Vec<Logger>){
    }
}
