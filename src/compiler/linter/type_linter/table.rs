//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §7 表构造 ==================

    /// `'{' fieldlist '}'`（表字面量）与 `'{' classfieldlist '}'`（类型位的匿名 record）。
    /// 两边走的是不同的列表非终结符，但形状和终点一样 —— 摊脊取元素、逐个分类。
    ///
    /// 出口不止 record 一种。三个桶各自成型、非空的交起来，塌不塌只看一条判据：
    /// **键空间重不重叠**。
    ///   - `array<T>` 的键是 number，`record` 的键是它列出的那几个字符串 —— 不重叠，
    ///     所以 `{1, x=2}` 出 `array<number> & record{x:number}`，两条投影都通：
    ///     `[i]` 走 as_array 扫交成员，`.x` 走 lookup_field
    ///   - `array<T>` 和 `table<K,V>` 在 K 含 number 时重叠，两边都能被 number 索，
    ///     交出来 as_array / as_map 都命中、`t[i]` 取谁没有说法 —— 所以这一对得合成
    ///     一个 `table<number|K, V>`，位置元素的类型并进 V
    ///   - record 一律**不**参与塌。塌下去就得往键里掺 `string`，那等于声称「任意
    ///     string 键都给 V」，可 `{1, x=2}` 里 `t["y"]` 在 Lua 里是 nil 而不是 V ——
    ///     那不只是变糊，是凭空发明一个不成立的许诺
    ///
    /// 于是七种组合都由这一条规则导出：单桶各出自己那种（类型位的匿名 record 永远
    /// 走 record，因为 classfield 四种形式全是静态的）；`record & table`（动机 A）；
    /// `array & record`；`table<number|K,V>`；`record & table<number|K,V>`
    pub(super) fn resolve_fields(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        site: FieldSite,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let list = ast.get_node(node_index).children[1];
        // 具名字段带上元素节点：重名诊断要报到出问题的那一项上，而不是整个表
        let mut named: Vec<(Field, usize)> = Vec::new();
        let mut positional = Vec::new();
        let mut dyn_keys = Vec::new();
        let mut dyn_vals = Vec::new();
        for item in self.spine(ast, list) {
            // classfieldlist 被类体和匿名 record 共用，可方法只在类体里说得通：
            // 类型位写一个带函数体的方法，那个体归谁、什么时候查都没有答案。
            // 要一个函数成员就写成属性 `m : function(…)`
            if site == FieldSite::Record && self.get_production(ast, item) == Some(Prod::MethodDef)
            {
                diags.push(Diagnostic {
                    span: ast.span_of(item),
                    msg: "类型位的 record 不能定义方法，函数成员请写成 m : function(…)".to_string(),
                });
            }
            match self.classify_field(ctx, ast, item) {
                Elem::Named(f) => named.push((f, item)),
                Elem::Positional(ty) => positional.push(ty),
                Elem::Dynamic { key, val } => {
                    dyn_keys.push(key);
                    dyn_vals.push(val);
                }
            }
        }
        let fields = self.dedup_named(ast, named, site, diags);
        let mut parts = Vec::with_capacity(2);
        if !fields.is_empty() {
            parts.push(self.session.type_arenas.record(fields));
        }
        match (!positional.is_empty(), !dyn_keys.is_empty()) {
            // 键空间重叠的那一对：number 进键，位置元素的类型并进值
            (true, true) => {
                let mut keys = vec![TypeId::NUMBER];
                keys.extend(dyn_keys);
                let mut vals = positional;
                vals.extend(dyn_vals);
                let k = self.session.type_arenas.union(keys);
                let v = self.session.type_arenas.union(vals);
                let tab = self.table_of(k, v);
                parts.push(tab);
            }
            (true, false) => {
                let elem = self.session.type_arenas.union(positional);
                let arr = self.array_of(elem);
                parts.push(arr);
            }
            (false, true) => {
                let k = self.session.type_arenas.union(dyn_keys);
                let v = self.session.type_arenas.union(dyn_vals);
                let tab = self.table_of(k, v);
                parts.push(tab);
            }
            (false, false) => {}
        }
        // 三桶全空由 @TableEmpty / @RecordEmpty 接走，正常到不了。真到了必须显式给空
        // record：intersect(vec![]) 返回的是 ANY，会把「空表」悄悄放成「什么都行」
        if parts.is_empty() {
            return self.session.type_arenas.record(vec![]);
        }
        // intersect() 不展开 Ref，所以 record 和容器就地留成两个成员；单成员时它自己
        // 折叠掉（`1 => rest[0]`），所以只有一个桶非空的情况不用特判
        self.session.type_arenas.intersect(parts)
    }

    /// 重名字段归一。必须在 `record()` 之前做掉：`intern_fields` 的 dedup 是为哈希
    /// 一致性服务的（`{a:number, a:number}` 得和 `{a:number}` 同一个 TypeId），它保留
    /// 第一个，正好和 Lua 相反。所以语义这一层得自己定：
    ///   - 值位（`{a=1, a=2}`）：Lua 是后写覆盖先写，取最后一个，不报错
    ///   - 类型位（`{a:number, a:string}`）：没有「覆盖」这回事，重名就是笔误，报错。
    ///     报完同样取最后一个继续往下跑 —— 不因为一个笔误连带出一串假错
    pub(super) fn dedup_named(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        named: Vec<(Field, usize)>,
        site: FieldSite,
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Field> {
        let mut at: HashMap<NameId, usize> = HashMap::new();
        let mut out: Vec<Field> = Vec::new();
        for (f, item) in named {
            match at.get(&f.name) {
                Some(&i) => {
                    if site == FieldSite::Record {
                        diags.push(Diagnostic {
                            span: ast.span_of(item),
                            msg: format!("字段 {} 重复声明", self.session.names.resolve(f.name)),
                        });
                    }
                    out[i] = f;
                }
                None => {
                    at.insert(f.name, out.len());
                    out.push(f);
                }
            }
        }
        out
    }

    /// 元素节点 -> 三种去处。先问 collect_field：它把字面量 string 键（`{["a"]=v}`）
    /// 也当静态具名字段收了，所以剩给下面那步 @FieldKV 的只有真正的动态键。
    /// 按键的**类型**而不是字面量判 number，所以 `{[1]=v}` 和 `{[i]=v}`（i:number）同归一类。
    ///
    /// 剩下那条就是位置字段：文法上 fields 侧只剩裸 exp（单元素那条被折叠所以
    /// 没标签），classfields 侧四种形式 collect_field 全接得住
    pub(super) fn classify_field(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
    ) -> Elem {
        if let Some(f) = self.collect_field(ctx, ast, node) {
            return Elem::Named(f);
        }
        if self.get_production(ast, node) == Some(Prod::FieldKV) {
            // '[' exp ']' '=' exp：键在 1、值在 4
            let key = self.child_type(ctx, ast.get_node(node).children[1]);
            let val = self.pass_through(ctx, ast, node, 4);
            return if key == TypeId::NUMBER {
                Elem::Positional(val)
            } else {
                Elem::Dynamic { key, val }
            };
        }
        Elem::Positional(self.child_type(ctx, node))
    }

    /// 列表的一项 -> record 字段。返回 None **只**表示「这不是一个具名字段」（动态键，
    /// 或无标签的裸 exp 位置字段），classify_field 靠这个约定把 None 归给位置桶。
    /// 所以名字取不到一律 panic 而不是返回 None —— 文法保证那几处就是 NAME，
    /// 静静返回 None 会把一个畸形的具名字段误标成数组元素，比当场炸难查得多
    ///
    /// 按**元素**标签 dispatch 而不是按父产生式，因为两侧的元素标签集不相交：
    /// fields 只出 @FieldNamed / @FieldKV / 裸 exp，classfields 只出 @FieldDecl /
    /// @MethodDef。`NAME '=' exp` 两边都能写，但表达式位归约成
    /// @FieldNamed、类型位归约成 @FieldDecl，文法已经把它们分开了，所以这个
    /// match 没有死分支，也不会让类型位放进本该被文法拦掉的形式
    pub(super) fn collect_field(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
    ) -> Option<Field> {
        match self.get_production(ast, node) {
            // NAME '=' exp：类型从 exp 推（@FieldNamed 已经把 children[2] 透上来了）
            Some(Prod::FieldNamed) => {
                let ty = self.child_type(ctx, node);
                let n = self.name_or_panic(ast, ast.get_node(node).children[0], "field name");
                let name = self.session.names.intern(n);
                // default 说的是**声明**有没有默认值，字面量里没这个概念。必须填 false：
                // 类型位的 record 一律 false（`= exp` 由 checker 拒绝），填 true 会让
                // `local t: {a:number} = {a=1}` 两边 intern 成不同 TypeId
                Some(Field {
                    name,
                    ty,
                    default: false,
                })
            }
            Some(Prod::FieldDecl) => Some(self.collect_field_decl(ctx, ast, node)),
            Some(Prod::FieldKV) => {
                // '[' exp ']' '=' exp：键在 children[1]、值在 children[4]
                let key_node = ast.get_node(node).children[1];
                match ast.get_node(key_node).get_data() {
                    // 字面量 string 键就是个静态字段名，和 `NAME = exp` 等价
                    NodeSyntax::Token(Token::STRING(s)) => Some(Field {
                        name: self.session.names.intern(&string_literal(s)),
                        // 字段类型取**值**。取键节点的类型是错的 —— 字面量 string 键
                        // 的类型恒为 string，`{["a"] = 5}` 会推成 `a: string`
                        ty: self.pass_through(ctx, ast, node, 4),
                        default: false,
                    }),
                    //动态字段：键不是字面量（`[k] = v`），没有静态名字就成不了具名
                    // Field，所以不给 record 贡献字段、直接跳过。检测是上层的事
                    _ => None,
                }
            }
            Some(Prod::MethodDef) => {
                let sig = ast.get_node(node).children[0];
                let n = self.name_or_panic(ast, ast.get_node(sig).children[0], "method name");
                let name = self.session.names.intern(n);
                // 取 @MethodDef 自己的类型而不是 methodsig 那一格：不标 `->` 时
                // 返回类型要拿体里的 return 推，而体比 methodsig 晚才跑完 ——
                // `resolve_method_def` 就是在这个节点上把推出来的返回列表装回签名的
                let ty = self.child_type(ctx, node);
                Some(Field {
                    name,
                    ty,
                    default: false,
                })
            }
            _ => None,
        }
    }

    /// @FieldDecl 三条产生式共用一个标签，孩子数也分不开（第一、三条都是 3 个），
    /// 得看 children[1] 是 ':' 还是 '='：
    ///   NAME ':' type           标注、无默认值
    ///   NAME ':' type '=' exp   标注、有默认值
    ///   NAME '=' exp            无标注，类型从 exp 推，算有默认值
    ///
    /// 返回 `(是不是标注, 有没有默认值)`。字段类型恒在 children[2]：
    /// 前两条是 type、第三条是 exp，两个读它的调用点口径因此一致
    pub(super) fn field_decl_shape(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> (bool, bool) {
        let children = &ast.get_node(node_index).children;
        let annotated = matches!(
            ast.get_node(children[1]).get_data(),
            NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE(':')))
        );
        (annotated, !annotated || children.len() > 3)
    }

    /// '{' classfieldlist '}' 的字段收集已并入 resolve_fields，这里只管单条 @FieldDecl
    pub(super) fn collect_field_decl(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> Field {
        let children = ast.get_node(node_index).children.clone();
        // 文法保证是 NAME。原来回退成 intern("") 会让多个畸形字段 dedup 成一个，
        // 比直接炸难查；和 resolve_generic_type / resolve_method_call 对齐
        let name = self.name_or_panic(ast, children[0], "field name");
        let name = self.session.names.intern(name);
        let (_, default) = self.field_decl_shape(ast, node_index);
        let ty = self.child_type(ctx, children[2]);
        Field { name, ty, default }
    }

    /// `array<T>`。内建 decl 已在 register_builtins 里占了 DeclId::ARRAY，直接拿它造 Ref：
    /// 不走 lookup_type —— 字面量推断不应该被用户自己声明的同名 `array` 遮蔽掉
    pub(super) fn array_of(&mut self, elem: TypeId) -> TypeId {
        let args = self.session.type_arenas.intern_list(vec![elem], None);
        self.session.type_arenas.reference(DeclId::ARRAY, args)
    }

    /// `table<K,V>`，同上
    pub(super) fn table_of(&mut self, key: TypeId, val: TypeId) -> TypeId {
        let args = self.session.type_arenas.intern_list(vec![key, val], None);
        self.session.type_arenas.reference(DeclId::TABLE, args)
    }

    // ---- 类名与构造（class 子系统的一部分）----
    //
    // 类名只活在**类型命名空间**（hoist_top_level_types 只调 declare_type），
    // 可 `A{…}` / `A.m(…)` 写在值位。所以凡是要认「这个裸名字是个类名」的
    // 地方（构造、`A.m = v` 的拒绝、接收者解析）都走下面这两条，口径才一致

    /// 名字 -> 它命名的那个 class（DeclId + 无实参的 Ref）。typedef / 标量 /
    /// 内建容器（array、table 没有值侧的那张表）/ 查不到，一律 None
    pub(super) fn class_ref_by_name(
        &mut self,
        ctx: &Context,
        name: &str,
    ) -> Option<(DeclId, TypeId)> {
        match self.lookup_type(ctx, name) {
            Some(TypeRef::Decl(decl))
                if !decl.is_builtin_container()
                    && self.session.decls.get(decl).kind() == DeclKind::Class =>
            {
                Some((
                    decl,
                    self.session.type_arenas.reference(decl, ListId::EMPTY),
                ))
            }
            _ => None,
        }
    }

    /// 这个节点是不是个「裸类名」：@VarRef、没有同名变量遮蔽、且确实命名一个 class。
    /// 先问变量表再问类型表 —— `local Local = {}` 之后 `Local(x)` 就是普通调用，
    /// 局部变量遮蔽类名和它遮蔽全局是同一回事
    pub(super) fn bare_class_ref(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
    ) -> Option<(DeclId, TypeId)> {
        if self.get_production(ast, node) != Some(Prod::VarRef) {
            return None;
        }
        let name = self.get_child_name(ast, node, 0)?;
        if self.lookup_variable(ctx, name).is_some() {
            return None;
        }
        self.class_ref_by_name(ctx, name)
    }

    /// 这条 class 声明是不是本模块自己写的。import 进来的类名只有类型：
    /// 对面那条 class 语句在别的文件里执行，本文件压根没那张表可以构造
    pub(super) fn class_is_local(&self, ctx: &Context, decl: DeclId) -> bool {
        let name = self.session.decls.get(decl).name();
        self.session.lookup_module_type(ctx.current_module, name) == Some(decl)
    }

    /// Ref 上那个 class 的名字，只进诊断文案
    pub(super) fn class_name(&self, class: TypeId) -> String {
        match self.session.type_arenas.get_type(class) {
            Types::Ref { decl, .. } => self
                .session
                .names
                .resolve(self.session.decls.get(decl).name())
                .to_string(),
            _ => String::new(),
        }
    }

    /// `prefixexp args` —— 调用，另外包一条**构造**：`A{…}` 里 A 是类名而
    /// 不是变量，走的是逐字段核对而不是函数调用（class 没有构造器，README）。
    /// 两条路靠 callee 分：裸类名 ⇒ 构造，其余一律当函数调
    pub(super) fn resolve_call(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let children = ast.get_node(node_index).children.clone();
        if let Some((decl, class)) = self.bare_class_ref(ctx, ast, children[0]) {
            return self.check_construction(ctx, ast, node_index, decl, class, diags);
        }
        let f = self.child_type(ctx, children[0]);
        let (args, spread) = self.call_args(ctx, ast, children[1]);
        let span = ast.span_of(node_index);
        // 推断得在核对之前：形参表里还挂着 `T` 的时候逐位比类型只会报假错
        let f = self.infer_call_generics(f, &args, span, diags);
        self.check_call_shape(f, &args, spread, span, diags);
        self.ret_of(f)
    }

    /// 调用点的实参：`(报错落点, 类型)` 一串，外加一位「末位摊不摊得开」。
    ///
    /// args 有四种形状：`()`、`(explist)`，以及 Lua 自带的两条糖 `f{…}` / `f"…"` ——
    /// 后两条折叠后 args 位直接就是那个表 / 字符串节点。末位实参的多值在 Lua 里
    /// 全都摊进实参表，Pack 带 vararg 时摊出几个算不出来，那一位就置上 spread
    pub(super) fn call_args(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        args: usize,
    ) -> (Vec<(Span, TypeId)>, bool) {
        let list = match self.get_production(ast, args) {
            Some(Prod::ArgsEmpty) => return (Vec::new(), false),
            Some(Prod::Args) => ast.get_node(args).children[1],
            // `f{…}` / `f"…"`：一个实参，就是这个节点自己
            _ => return (vec![(ast.span_of(args), self.child_type(ctx, args))], false),
        };
        let nodes = self.spine(ast, list);
        let mut out = Vec::new();
        let mut spread = false;
        for (i, &node) in nodes.iter().enumerate() {
            let ty = self.child_type(ctx, node);
            let at = ast.span_of(node);
            // 非末位一律截成一个值；末位才摊
            if i + 1 < nodes.len() {
                out.push((at, self.truncate_ret(ty)));
                continue;
            }
            let (items, tail) = self.value_items(ty, at);
            out.extend(items);
            spread = tail;
        }
        (out, spread)
    }

    /// 调用点的类型实参推断：`map(xs, string.len)` 不写 `::<string, number>`
    /// 也得知道 T=string、U=number（README「泛型」）。
    ///
    /// 做法是形参表和实参表逐位比形状，比到形参位上那个 `T` 就把对面的类型记
    /// 下来，最后一次性代入。推得出几个代几个 —— `subst` 只把代掉的那几个从
    /// generics 那格摘走，剩下的还是形参，该由 `::<>` 补的照旧补得上。
    ///
    /// 一个形参只认第一次命中：`f(a:T, b:T)` 给了 number 和 string 时不去猜哪个
    /// 才对，留着让 `check_positional` 按 T=number 去报第二个实参 —— 那条诊断
    /// 比一句「推断失败」有用得多
    pub(super) fn infer_call_generics(
        &mut self,
        callee: TypeId,
        args: &[(Span, TypeId)],
        span: Span,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let Types::Func {
            generics, params, ..
        } = self.session.type_arenas.get_type(callee)
        else {
            return callee;
        };
        // 和 turbofish 同一步：generics 那格存的是 Generic 包过的 TypeId
        let gids: Vec<GenericId> = self
            .session
            .type_arenas
            .list(generics)
            .fixed
            .iter()
            .filter_map(|&g| match self.session.type_arenas.get_type(g) {
                Types::Generic(id) => Some(id),
                _ => None,
            })
            .collect();
        if gids.is_empty() {
            return callee;
        }
        let want = self.session.type_arenas.list(params).clone();
        let mut sol: Vec<Option<TypeId>> = vec![None; gids.len()];
        for (i, &(_, got)) in args.iter().enumerate() {
            let Some(w) = want.fixed.get(i).copied().or(want.vararg) else {
                break;
            };
            self.unify_generic(w, got, &gids, &mut sol);
        }
        let (ps, tys): (Vec<GenericId>, Vec<TypeId>) = gids
            .iter()
            .zip(&sol)
            .filter_map(|(&g, &t)| t.map(|t| (g, t)))
            .unzip();
        if ps.is_empty() {
            return callee;
        }
        // 上界照样管推出来的实参：`<T : number>` 传了 string，错在调用点
        self.check_generic_bounds(&ps, &tys, span, diags);
        let s = self.session.type_arenas.intern_subst(&ps, &tys);
        self.session.type_arenas.subst(callee, s)
    }

    /// 拿形参位的形状去套实参的形状，套到 `T` 就记一格。
    ///
    /// 只往「同构」的方向下钻：同一个 decl 的 Ref 比实参、函数比形参和返回列表、
    /// record 比同名字段。联类型不下钻 —— `T|nil` 对上 `string|nil` 该解出
    /// T=string 还是 T=string|nil 没有唯一答案，宁可推不出来让人写 `::<>`。
    ///
    /// unknown 不往格里填：那是「这个实参前面已经错了」，填进去等于把整条调用的
    /// 结果也污染成 unknown，后面就全不检查了
    pub(super) fn unify_generic(
        &mut self,
        want: TypeId,
        got: TypeId,
        gids: &[GenericId],
        sol: &mut [Option<TypeId>],
    ) {
        if !self.session.type_arenas.contains_generic(want) || got == TypeId::UNKNOWN {
            return;
        }
        if let Types::Generic(g) = self.session.type_arenas.get_type(want) {
            if let Some(i) = gids.iter().position(|&p| p == g) {
                sol[i] = sol[i].or(Some(got));
            }
            return;
        }
        // 别名先展开才比得上：`typedef Names = array<string>` 传给 `array<T>`
        let got = self.resolve_alias(got);
        match (
            self.session.type_arenas.get_type(want),
            self.session.type_arenas.get_type(got),
        ) {
            (Types::Ref { decl: a, args: x }, Types::Ref { decl: b, args: y }) if a == b => {
                self.unify_lists(x, y, gids, sol)
            }
            (
                Types::Func {
                    params: p1,
                    ret: r1,
                    ..
                },
                Types::Func {
                    params: p2,
                    ret: r2,
                    ..
                },
            ) => {
                self.unify_lists(p1, p2, gids, sol);
                self.unify_lists(r1, r2, gids, sol);
            }
            (Types::Record(f1), Types::Record(f2)) => {
                let wants = self.session.type_arenas.fields(f1).to_vec();
                let gots = self.session.type_arenas.fields(f2).to_vec();
                for w in wants {
                    if let Some(g) = gots.iter().find(|f| f.name == w.name).copied() {
                        self.unify_generic(w.ty, g.ty, gids, sol);
                    }
                }
            }
            (Types::Pack(a), Types::Pack(b)) => self.unify_lists(a, b, gids, sol),
            _ => {}
        }
    }

    /// 两条列表逐位套。给的那边短了就拿它的 vararg 顶上（`function(…)` 的形参
    /// 表比 `function(T)` 长不了，但传进来的函数值可能是变长的）
    pub(super) fn unify_lists(
        &mut self,
        want: ListId,
        got: ListId,
        gids: &[GenericId],
        sol: &mut [Option<TypeId>],
    ) {
        let wants = self.session.type_arenas.list(want).clone();
        let gots = self.session.type_arenas.list(got).clone();
        for (i, &w) in wants.fixed.iter().enumerate() {
            if let Some(&g) = gots.fixed.get(i).or(gots.vararg.as_ref()) {
                self.unify_generic(w, g, gids, sol);
            }
        }
        if let (Some(w), Some(g)) = (wants.vararg, gots.vararg) {
            self.unify_generic(w, g, gids, sol);
        }
    }

    /// 调用点核对：先看 callee 调不调得动，再把实参逐位塞进形参表。
    ///
    /// 「不是函数还调它」只拦确定不可调的标量：record / class 在 Lua 里能挂
    /// `__call` 元表，该不该报得单论，拿不准就放过
    pub(super) fn check_call_shape(
        &mut self,
        callee: TypeId,
        args: &[(Span, TypeId)],
        spread: bool,
        span: Span,
        diags: &mut Vec<Diagnostic>,
    ) {
        let Types::Func { params, .. } = self.session.type_arenas.get_type(callee) else {
            if matches!(
                callee,
                TypeId::NIL | TypeId::NUMBER | TypeId::BOOLEAN | TypeId::STRING
            ) {
                diags.push(Diagnostic {
                    span,
                    msg: format!("不能调用 {} 类型的值", self.session.show(callee)),
                });
            }
            return;
        };
        let params = self.session.type_arenas.list(params).clone();
        self.check_positional(args, &params, spread, span, "实参", diags);
    }

    /// 一串值逐位塞进一串位子。实参和 return 值共用 —— 形参表和返回列表
    /// 都是 `TypeList`，规矩一模一样，差的只有诊断里的位置名（`kind`）。
    ///
    /// 个数分两头。多给了、而那串位子没 vararg 收，就报。少给了要看缺的那几位
    /// 收不收 nil —— Lua 里没传的形参就是 nil，所以 `p : number|nil` 少传是合法的，
    /// return 少给同理。`spread` = 给的那串末位是变长（`f(g())` / `return g()`），
    /// 摊出几个算不出来，个数那半跳过、类型照核
    pub(super) fn check_positional(
        &mut self,
        got: &[(Span, TypeId)],
        want: &TypeList,
        spread: bool,
        span: Span,
        kind: &str,
        diags: &mut Vec<Diagnostic>,
    ) {
        for (i, &(at, g)) in got.iter().enumerate() {
            let Some(w) = want.fixed.get(i).copied().or(want.vararg) else {
                break;
            };
            self.expect_assignable(g, w, at, &format!("第 {} 个{kind}", i + 1), diags);
        }
        if spread {
            return;
        }
        let wanted = want.fixed.len();
        let missing_needs_value = want
            .fixed
            .iter()
            .skip(got.len())
            .any(|&w| !self.param_takes_nil(w));
        if (got.len() > wanted && want.vararg.is_none()) || missing_needs_value {
            diags.push(Diagnostic {
                span,
                msg: format!("{kind}个数不对：要 {wanted} 个，给了 {} 个", got.len()),
            });
        }
    }

    /// 这个位子能不能干脆不给值。泛型形参不算 —— `T` 拿不准能不能是 nil，
    /// 而要是拿不准就当可省，`id<T>(x:T)` 写成 `id()` 就没人拦了
    pub(super) fn param_takes_nil(&mut self, want: TypeId) -> bool {
        !matches!(self.session.type_arenas.get_type(want), Types::Generic(_))
            && self.session.assignable(TypeId::NIL, want)
    }

    /// `A{…}` —— 类没有构造器，构造就是「按字段核对一张表」。
    ///
    /// 为什么不复用 `assignable`：class 是名义类型，`assignable(record, Ref A)`
    /// 按设计恒 false（不然任何形状对得上的表都能冒充 A）。构造是唯一的例外，
    /// 得自己逐字段对：
    ///   - 只认具名项：位置元素 / 动态键给不出字段名
    ///   - 方法不许给：方法必在类体里定义（`m(self) … end`），构造里给一个同名
    ///     函数不是「实现方法」而是偷换；要一个能逐实例不同的函数成员就把它声明成
    ///     函数变量字段 `m : function(…)`
    ///   - 没默认值的变量字段每处构造都得给；有默认值的可省
    ///
    /// 结果类型无论对错都是那个 Ref：字段给错不影响「造出来的是个 A」，
    /// 回 UNKNOWN 只会让后续每一处用它的地方再堆一堆假错
    pub(super) fn check_construction(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        call: usize,
        decl: DeclId,
        class: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let span = ast.span_of(call);
        let cname = self.class_name(class);
        if !self.class_is_local(ctx, decl) {
            diags.push(Diagnostic {
                span,
                msg: format!("{cname} 是 import 来的类，只有类型没有值，不能用它构造"),
            });
            return class;
        }
        // `args : tableconstructor` 是无标签单孩子、被折叠了，所以这里直接就是表节点
        let arg = ast.get_node(call).children[1];
        let list = match self.get_production(ast, arg) {
            Some(Prod::TableEmpty) => None,
            Some(Prod::Table) => Some(ast.get_node(arg).children[1]),
            // `A(…)` / `A"…"`：类名不是函数，只有表形式算构造
            _ => {
                diags.push(Diagnostic {
                    span,
                    msg: format!("class {cname} 只能用 {cname}{{…}} 构造"),
                });
                return class;
            }
        };
        let fields = self.class_fields_flat(class);
        let mut given: Vec<NameId> = Vec::new();
        for item in list.map(|l| self.spine(ast, l)).unwrap_or_default() {
            let item_span = ast.span_of(item);
            let Elem::Named(f) = self.classify_field(ctx, ast, item) else {
                diags.push(Diagnostic {
                    span: item_span,
                    msg: format!("构造 {cname} 只能用 字段名 = 值，位置元素和动态键都给不出字段名"),
                });
                continue;
            };
            given.push(f.name);
            let shown = self.session.names.resolve(f.name).to_string();
            match fields.iter().find(|e| e.name == f.name).copied() {
                None => diags.push(Diagnostic {
                    span: item_span,
                    msg: format!("{shown} 不是 {cname} 的字段"),
                }),
                Some(hit) if hit.method => diags.push(Diagnostic {
                    span: item_span,
                    msg: format!("{shown} 是方法，方法只能在类体里定义，不能在构造里给"),
                }),
                Some(hit) => {
                    if !self.session.assignable(f.ty, hit.ty) {
                        diags.push(Diagnostic {
                            span: item_span,
                            msg: format!(
                                "不能把 {} 赋给字段 {shown} 标注的 {} 位",
                                self.session.show(f.ty),
                                self.session.show(hit.ty)
                            ),
                        });
                    }
                }
            }
        }
        // 没给的字段报在整个构造式上 —— 漏给一个字段没有可指的子节点
        for hit in fields {
            if hit.default || hit.method || given.contains(&hit.name) {
                continue;
            }
            let shown = self.session.names.resolve(hit.name).to_string();
            diags.push(Diagnostic {
                span,
                msg: format!("构造 {cname} 缺少字段 {shown}：它没有默认值"),
            });
        }
        class
    }
}
