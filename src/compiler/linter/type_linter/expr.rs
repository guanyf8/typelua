//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §6 表达式 ==================

    /// `op_child` 是操作符 token 所在的下标：BinOp 在中间（1）、UnOp 在最前（0）。
    /// 它同时也是 arity —— `-` 和 `~` 一元二元同字符但元方法不同，得靠它分开。
    ///
    /// 两条路：操作数都是标量走内建结果（Lua 在 number/string 之间自动强转）；
    /// 否则要求两侧**同类型**、且那个类型带对应元方法，然后返回同类型。
    /// class 查 fields、table 查交进来的元表 record，两者是 `lookup_field` 的同一条路
    pub(super) fn resolve_operator(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        op_child: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let op = ast.get_node(node_index).children[op_child];
        let unary = op_child == 0;
        let NodeSyntax::Token(op_token) = ast.get_node(op).get_data() else {
            return TypeId::UNKNOWN;
        };
        // 内建出口：操作数都是标量时的结果，也是不查元表那些运算符的唯一出口
        let builtin = match op_token {
            Token::RESERVED(Reserved::AND | Reserved::OR | Reserved::NOT) => TypeId::BOOLEAN,
            Token::OPERATOR(OpType::EQ | OpType::NE | OpType::LE | OpType::GE) => TypeId::BOOLEAN,
            Token::OPERATOR(OpType::SIMPLE('<' | '>')) => TypeId::BOOLEAN,
            Token::OPERATOR(
                OpType::SIMPLE('+' | '-' | '*' | '/' | '%' | '^' | '#' | '~') | OpType::IDIV,
            ) => TypeId::NUMBER,
            Token::OPERATOR(OpType::SHL | OpType::SHR) => TypeId::NUMBER,
            Token::OPERATOR(OpType::CONCAT) => TypeId::STRING,
            _ => TypeId::UNKNOWN,
        };
        let Some((mm, kind)) = metamethod(op_token, unary) else {
            return builtin;
        };
        // 操作数位可能是多值（`f() + 1`），截到一个
        let lhs =
            self.truncate_ret(self.pass_through(ctx, ast, node_index, if unary { 1 } else { 0 }));
        let span = ast.span_of(node_index);
        let operand = if unary {
            lhs
        } else {
            let rhs = self.truncate_ret(self.pass_through(ctx, ast, node_index, 2));
            // 两侧都是标量就不必同类型：`1 .. "a"`、`"2" * 3` 在 Lua 里都合法
            if self.is_primitive_operand(lhs) && self.is_primitive_operand(rhs) {
                return builtin;
            }
            if lhs != rhs {
                diags.push(Diagnostic {
                    span,
                    msg: format!(
                        "{} 运算要求两侧同类型，实际是 {} 和 {}",
                        mm,
                        self.session.show(lhs),
                        self.session.show(rhs)
                    ),
                });
                return TypeId::UNKNOWN;
            }
            lhs
        };
        if self.is_primitive_operand(operand) {
            return builtin;
        }
        // class 走 Ref 进 fields（带 extends 链）、`table<K,V> & {__add:...}` 走 Intersect
        // 逐成员查 —— 用户要的「class 看 field」和「table 看元表」在 lookup_field 里是
        // 同一条路，不用分开写
        let name = self.session.names.intern(mm);
        if self.lookup_field(operand, name).is_none() {
            diags.push(Diagnostic {
                span,
                msg: format!(
                    "{} 上没有 {} 元方法，不支持这个运算",
                    self.session.show(operand),
                    mm
                ),
            });
            // 给 UNKNOWN 而不是 builtin：这里已经报过一次，回落成 number 会让后面
            // 拿着一个假类型继续算，错报到别处去
            return TypeId::UNKNOWN;
        }
        match kind {
            MetaResult::SameAsOperand => operand,
            MetaResult::Fixed => builtin,
        }
    }

    /// 不查元表就能算的操作数。number/string 之间 Lua 的算术和 `..` 会自动强转；
    /// any/unknown 是渐进类型的逃逸口；Generic 要等实例化才知道有没有元方法。
    /// 和 `expect_key` 同一个尺度：宁可漏报，也不在类型信息还不全的阶段刷误报
    pub(super) fn is_primitive_operand(&self, ty: TypeId) -> bool {
        matches!(
            ty,
            TypeId::NUMBER | TypeId::STRING | TypeId::ANY | TypeId::UNKNOWN
        ) || matches!(self.session.type_arenas.get_type(ty), Types::Generic(_))
    }

    /// `prefixexp '[' exp ']'` —— 只有内建容器能静态定出元素类型。
    /// record 没有字面量类型（算不出键到底是哪个字段）、class 字段该走 `.`，
    /// 所以它们和 any 一样给 ANY：渐进类型的逃逸口，不是 UNKNOWN
    pub(super) fn resolve_index(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        // 别名透明：`typedef Xs = array<number>` 上的 `xs[1]` 和直接写 array 一样
        let base = self.resolve_alias(self.pass_through(ctx, ast, node_index, 0));
        // `t[f()]`：键位不是末位，多值要截到一个
        let key = self.truncate_ret(self.pass_through(ctx, ast, node_index, 2));
        let span = ast.span_of(node_index);
        if let Some(elem) = self.session.type_arenas.as_array(base) {
            self.expect_key(key, TypeId::NUMBER, span, diags, "数组下标");
            return elem;
        }
        if let Some((k, v)) = self.session.type_arenas.as_map(base) {
            self.expect_key(key, k, span, diags, "表键");
            return v;
        }
        // 剩下的里面有确定不是表的：索引 nil / number / boolean 在 Lua 里是运行时错误
        // （string 不拦 —— 它有元表，`s[1]` 只是 nil 而不报错）
        if matches!(base, TypeId::NIL | TypeId::NUMBER | TypeId::BOOLEAN) {
            diags.push(Diagnostic {
                span,
                msg: format!("不能索引 {} 类型的值", self.session.show(base)),
            });
        }
        TypeId::ANY
    }

    /// 键类型的粗检。真正的可赋值判定（union / 名义子类型 / 宽度子类型）要等
    /// assignable，这里只拦「写错得很明白」的：any / unknown / 泛型形参一律放过，
    /// 否则在类型信息还不全的阶段会刷一屏误报
    pub(super) fn expect_key(
        &self,
        got: TypeId,
        want: TypeId,
        span: Span,
        diags: &mut Vec<Diagnostic>,
        what: &str,
    ) {
        if got == want || matches!(got, TypeId::ANY | TypeId::UNKNOWN) {
            return;
        }
        if matches!(self.session.type_arenas.get_type(got), Types::Generic(_)) {
            return;
        }
        diags.push(Diagnostic {
            span,
            msg: format!(
                "{}应为 {}，实际是 {}",
                what,
                self.session.show(want),
                self.session.show(got)
            ),
        });
    }

    /// 可赋值性核对 + 报错。`what` 描述的是「这个位子是谁」（字段 f / 元素 …）
    pub(super) fn expect_assignable(
        &mut self,
        got: TypeId,
        want: TypeId,
        span: Span,
        what: &str,
        diags: &mut Vec<Diagnostic>,
    ) {
        if self.session.assignable(got, want) {
            return;
        }
        diags.push(Diagnostic {
            span,
            msg: format!(
                "不能把 {} 赋给{}的 {} 位",
                self.session.show(got),
                what,
                self.session.show(want)
            ),
        });
    }

    /// 形状拿不准的拥有者：any / unknown 这两个口子，和还没实例化的泛型形参。
    /// 字段存不存在这类判定对它们一律放过 —— 和 `expect_key` 同一个尺度
    pub(super) fn is_opaque_owner(&self, ty: TypeId) -> bool {
        matches!(ty, TypeId::ANY | TypeId::UNKNOWN)
            || matches!(self.session.type_arenas.get_type(ty), Types::Generic(_))
    }

    /// 能拿它报「没这个字段」的那几种形状：record、class / 容器的 Ref、交类型。
    /// 得先 `resolve_alias` 过。union 不算 —— 它的字段该不该取交集是个单独的设计题；
    /// 标量与函数也不算 —— 它们在 Lua 里有元表，该不该报得单论
    pub(super) fn is_known_shape(&self, ty: TypeId) -> bool {
        !self.is_opaque_owner(ty)
            && matches!(
                self.session.type_arenas.get_type(ty),
                Types::Record(_) | Types::Ref { .. } | Types::Intersect(_)
            )
    }

    /// 沿 typedef 链展开到一个不是别名的类型。形状判定（是不是容器、是不是
    /// record）都得先过这一道 —— 别名是透明的，`typedef M = table<string,number>`
    /// 和它展开后那个类型在语义上是同一回事。循环别名靠圈数兜底
    pub(super) fn resolve_alias(&mut self, ty: TypeId) -> TypeId {
        let mut cur = ty;
        for _ in 0..64 {
            match self.session.expand_typedef(cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
        cur
    }

    /// 在 owner 上查名为 name 的字段/方法，返回**已代入实参**的类型。
    /// 字段类型存的是类自己的泛型形参，所以查到就得 subst。
    /// 沿 extends 链上溯时，父类的 Ref 实参先用子类的替换过一遍 ——
    /// `class Flipped<A,B> : Pair<B,A>` 的重排靠这一步才不丢。
    /// resolve_dot / resolve_index / resolve_method_call 也都该走它
    pub(super) fn lookup_field(&mut self, owner: TypeId, name: NameId) -> Option<TypeId> {
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
                    let s = self.session.ref_subst(decl, args);
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

    /// owner 上那个名字的类型：先查声明的字段，查不到再退到容器按键取。
    /// 「有没有这个成员」在字段读和方法调用两处得是同一个答案，所以走同一条
    pub(super) fn member_type(&mut self, owner: TypeId, name: NameId) -> Option<TypeId> {
        if let Some(ty) = self.lookup_field(owner, name) {
            return Some(ty);
        }
        let shape = self.resolve_alias(owner);
        self.session.type_arenas.as_map(shape).map(|(_, v)| v)
    }

    /// 调用一个可调用类型得到的值。ret 是列表，单个会被 pack_of 折回裸类型
    pub(super) fn ret_of(&mut self, callee: TypeId) -> TypeId {
        match self.session.type_arenas.get_type(callee) {
            Types::Func { ret, .. } => self.session.type_arenas.pack_of(ret),
            _ => TypeId::UNKNOWN,
        }
    }

    /// 沿 extends 链找一个类字段，把「是不是方法」一起带回来（父类实参已代入）。
    ///
    /// 为什么不复用 `lookup_field`：那条只给类型，而「方法 vs 函数变量字段」这一位
    /// 存在**声明层**的 `ClassField` 上、故意不进类型层（见那里的注释）。构造检查和
    /// 类体外重定义都得先分清这一位，所以另开一条只走 class 的路 —— typedef 不展开：
    /// 别名后面的 record 是结构类型，压根没有方法这个概念
    pub(super) fn lookup_class_field(
        &mut self,
        owner: TypeId,
        name: NameId,
    ) -> Option<(TypeId, bool)> {
        let mut cur = owner;
        // 和 lookup_field 同一个道理：成环该由 check_class_decl_extends 报，这里只保证不挂死
        for _ in 0..64 {
            let Types::Ref { decl, args } = self.session.type_arenas.get_type(cur) else {
                return None;
            };
            let s = self.session.ref_subst(decl, args);
            let (hit, next) = match &self.session.decls.get(decl).body {
                DeclBody::Class(c) => (
                    c.fields
                        .iter()
                        .find(|f| f.field.name == name)
                        .map(|f| (f.field.ty, f.method)),
                    c.extends,
                ),
                DeclBody::Typedef(_) => return None,
            };
            match hit {
                Some((ty, method)) => return Some((self.session.type_arenas.subst(ty, s), method)),
                None => cur = self.session.type_arenas.subst(next?, s),
            }
        }
        None
    }

    /// 构造 `A{…}` 要核对的全部字段：本类的 + 沿 extends 链继承来的（父类实参已代入）。
    /// 子类先入、重名时只留子类那份 —— 覆盖本该被 `check_field_overrides` 拦掉，
    /// 这里去重只是不让同一个字段被要求两遍
    pub(super) fn class_fields_flat(&mut self, owner: TypeId) -> Vec<FlatField> {
        let mut out: Vec<FlatField> = Vec::new();
        let mut cur = owner;
        for _ in 0..64 {
            let Types::Ref { decl, args } = self.session.type_arenas.get_type(cur) else {
                break;
            };
            let s = self.session.ref_subst(decl, args);
            let (own, next) = match &self.session.decls.get(decl).body {
                DeclBody::Class(c) => (
                    c.fields
                        .iter()
                        .map(|f| FlatField {
                            name: f.field.name,
                            ty: f.field.ty,
                            default: f.field.default,
                            method: f.method,
                        })
                        .collect::<Vec<_>>(),
                    c.extends,
                ),
                DeclBody::Typedef(_) => break,
            };
            for mut f in own {
                if out.iter().any(|e| e.name == f.name) {
                    continue;
                }
                f.ty = self.session.type_arenas.subst(f.ty, s);
                out.push(f);
            }
            match next {
                Some(p) => cur = self.session.type_arenas.subst(p, s),
                None => break,
            }
        }
        out
    }

    /// prefixexp ':' NAME args —— 方法调用。
    ///
    /// ':' 形式把接收者当第一个实参传进去，而 self 是写在形参表里的，所以
    /// 核对时把接收者拼在实参表头上。形状确定却查不到这个名字就报 ——
    /// 和字段读同一条规矩
    pub(super) fn resolve_method_call(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let children = ast.get_node(node_index).children.clone();
        //prefix是class/table/array(后两者都是class)没跑了
        let recv = self.child_type(ctx, children[0]);
        let mname = self
            .name_or_panic(ast, children[2], "method name")
            .to_string();
        let name = self.session.names.intern(&mname);
        let Some(m) = self.member_type(recv, name) else {
            let shape = self.resolve_alias(recv);
            if self.is_known_shape(shape) {
                diags.push(Diagnostic {
                    span: ast.span_of(node_index),
                    msg: format!("{} 上没有方法 {mname}", self.session.show(shape)),
                });
            }
            return TypeId::UNKNOWN;
        };
        let mut args = vec![(ast.span_of(children[0]), recv)];
        let (rest, spread) = self.call_args(ctx, ast, children[3]);
        args.extend(rest);
        self.check_call_shape(m, &args, spread, ast.span_of(node_index), diags);
        self.ret_of(m)
    }

    /// `prefixexp : NAME` —— 变量位的裸名字，全 linter 唯一查 variables 的地方。
    /// 顺序是内层作用域 -> 外层 -> Session 的全局，「局部遮蔽全局」就是它。
    ///
    /// 这里**不报**诊断，两条都得留给 `enter`：
    ///   - 「未声明」：赋值目标（`G:Config = {…}` 的 `G`）也是这个节点，而那正是
    ///     全局的声明点。`is_write_target` 分得开读写，但声明动作本身也在 `enter`，
    ///     后序遍历跑到这儿时表里还是空的
    ///   - 「可能尚未赋值」：得知道自己在不在函数体内 —— 延迟求值位置不查，否则
    ///     `a = function() b() end` 这种互递归的前向声明立刻误报，而 funcbody
    ///     的进出只有前序遍历数得清
    pub(super) fn resolve_var_ref(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        _diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let name = self.name_or_panic(ast, ast.get_node(node_index).children[0], "variable");
        self.lookup_variable(ctx, name)
            .map_or(TypeId::UNKNOWN, |slot| slot.ty)
    }

    /// `prefixexp : '(' exp ')'` —— Lua 的括号把多值截成一个值（`(f())` 只留
    /// 第一个），所以不是直接透传：取里面那个表达式再走一趟 truncate。
    /// `local e = (error)` 之后 `e("x")` 能认出 never，就靠这一步不把类型丢成 UNKNOWN
    pub(super) fn resolve_paren(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        self.truncate_ret(self.pass_through(ctx, ast, node_index, 1))
    }

    /// `prefixexp '.' NAME` —— 字段读。走 `lookup_field`（class 查 fields、record
    /// 查字段、交类型依次找），命中返回已代入实参的字段类型。`os.exit()` 能被
    /// 判成终结，前提就是这里把 `os.exit` 解析成那个 `-> never` 的函数类型。
    ///
    /// 基是类名时（`A.make(1)`）拿本类的 Ref 当基 —— 类名不是变量，但静态
    /// 成员就是这么读的。形状确定却查不到就报：表和 class 的形状一次定死，
    /// 读一个不存在的字段永远只能拿到 nil，那就不是「还没算出来」而是写错了
    pub(super) fn resolve_dot(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let children = ast.get_node(node_index).children.clone();
        let fname = self
            .name_or_panic(ast, children[2], "field name")
            .to_string();
        let name = self.session.names.intern(&fname);
        let base = match self.bare_class_ref(ctx, ast, children[0]) {
            Some((_, class)) => class,
            None => self.child_type(ctx, children[0]),
        };
        if let Some(ty) = self.lookup_field(base, name) {
            return ty;
        }
        // `table<K,V>` 按名字读就是读键为 "k" 那一项，和写位同一条规矩。
        // 写位的诊断归 `check_field_write` —— 那边才知道要写进去的值是什么类型，
        // 两边都报就变成同一个错报两道
        let shape = self.resolve_alias(base);
        let writing = self.is_writing(ast, node_index);
        if let Some((k, v)) = self.session.type_arenas.as_map(shape) {
            if !writing {
                self.expect_key(TypeId::STRING, k, ast.span_of(node_index), diags, "表键");
            }
            return v;
        }
        if writing || !self.is_known_shape(shape) {
            return TypeId::UNKNOWN;
        }
        diags.push(Diagnostic {
            span: ast.span_of(node_index),
            msg: format!("{} 上没有字段 {fname}", self.session.show(shape)),
        });
        TypeId::UNKNOWN
    }

    /// `var : prefixexp optype` —— 赋值目标。它的类型就是被赋的那个 prefixexp；
    /// optype 只是贴在裸名字上的标注（`G:Config = …`），归 `check_assign` 处理，
    /// 不进这里的类型
    pub(super) fn resolve_var(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        self.pass_through(ctx, ast, node_index, 0)
    }
}
