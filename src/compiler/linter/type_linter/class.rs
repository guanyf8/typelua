//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §10 类与方法 ==================
    //
    // class 子系统。先声明层（填体、继承、字段收集与覆盖检查），再方法 ——
    // 类体内的 @MethodDef 和类体外的 `function A:m` / `function A.m`。
    // 泛型形参在 P0a 就铸好了身份，实参代入走 Session::ref_subst；显式类型实参
    // （turbofish）与形参上界在 §8、调用点推断在 §7，均已落地

    // ---- 类与类型别名 ----
    //
    // 两阶段注册的第二步：名字已在 prepare 的 hoist_top_level_types 里占好
    // （空体、泛型形参也在那趟铸好身份），这里把体填上 —— 所以体里对自己 /
    // 互相 / 前向的引用都已解析成 Ref，体里的泛型形参 `T` 靠 enter/eager 押入的
    // 那格作用域解析成 Generic。方法（@MethodDef）此刻按签名类型的字段收着，
    // 真正的 self 绑定归下面那小节

    /// 顶层声明名字 -> 它在 P0a 抬升时拿到的 DeclId。非顶层（嵌在 do/if 里）的
    /// class/typedef 名字没被抬升，lookup 要么落空要么撞上同名的外层声明，填错体
    /// 比不填更糟，所以这些直接跳过 —— 名义类型只认顶层，和全局声明点一个道理
    pub(super) fn hoisted_decl(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        decl_node: usize,
        name: &str,
    ) -> Option<DeclId> {
        if !self.is_chunk_top(ast, decl_node) {
            return None;
        }
        match self.lookup_type(name) {
            Some(TypeRef::Decl(decl)) => Some(decl),
            _ => None,
        }
    }

    /// 顶层同名声明只有第一条算数：hoist 时 declare_type 只认第一条，DeclId
    /// 记的是第一条声明处 NAME 的 span。后来的同名声明（class 撞 class，也可能
    /// class 撞 typedef）本节点的 NAME span 和它对不上，就是重复 —— 报在本节点上，
    /// 且不去填那条已被第一条占住的 DeclId 的体，否则后一条的字段会把第一条覆盖掉
    pub(super) fn is_dup_decl(&self, decl: DeclId, name_span: Span) -> bool {
        self.session.decls.get(decl).span() != name_span
    }

    /// class / typedef 三条声明填体前共用的前导：拿到 P0a 给这个名字的 DeclId，
    /// 顺手把重复声明挡掉。`None` = 这条不该填体（非顶层、没抬升过，或它是重复的那条）。
    /// 名字一并带回来 —— 后面的诊断消息都要用
    pub(super) fn decl_to_fill<'t>(
        &self,
        ast: &'t Tree<NodeSyntax<'t>>,
        node_index: usize,
        name_node: usize,
        what: &str,
        log: &mut Vec<Logger>,
    ) -> Option<(DeclId, &'t str)> {
        let name = self.name_or_panic(ast, name_node, what);
        // 嵌在 do / if / 函数体里的 class/typedef 不是声明点：P0a 没抬升它，名义类型
        // 只认顶层（和 extern / import / pub 同理，文法表达不了、都在 checker 拦）。
        // 从前这里跟着 hoisted_decl 一起静默跳过，引用处只会撞上「未声明的类型」，
        // 错得不知所以 —— 现在就地报一条明确诊断。注意别下沉到 hoisted_decl：它还被
        // scope_decl_generics 复用，那条路径对非顶层就该静默，报在这里才不会重出
        if !self.is_chunk_top(ast, node_index) {
            log.push(Logger {
                span: ast.span_of(name_node),
                msg: "class / typedef 只能写在文件顶层".to_string(),
            });
            return None;
        }
        let decl = self.hoisted_decl(ast, node_index, name)?;
        if self.is_dup_decl(decl, ast.span_of(name_node)) {
            log.push(Logger {
                span: ast.span_of(name_node),
                msg: format!("类型名 {name} 重复声明"),
            });
            return None;
        }
        Some((decl, name))
    }

    /// classbody 的字段，带源码位置、按 NameId 去重。classfield 四种形式
    /// （字段声明 / 两种带默认值 / 方法定义）collect_field 全接得住；重名是笔误，
    /// 报在出问题那一项上、取最后一个继续（和类型位 record 的 dedup 同策略）。
    ///
    /// `method` 位按**形式**定：只有 @MethodDef（`m(self) … end`）是方法，
    /// `m : function(…)` 写出来的是函数变量字段。两者不互为糖，后面构造、
    /// 类体外定义、字段写入三处都要靠它分路
    pub(super) fn collect_class_fields(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        classbody: usize,
        log: &mut Vec<Logger>,
    ) -> Vec<ClassField> {
        let children = ast.get_node(classbody).children.clone();
        // `'{' '}'`：空体，len == 2；`'{' classfieldlist '}'`：list 在 children[1]
        if children.len() < 3 {
            return Vec::new();
        }
        let mut at: HashMap<NameId, usize> = HashMap::new();
        let mut out: Vec<ClassField> = Vec::new();
        for item in self.spine(ast, children[1]) {
            let method = self.get_production(ast, item) == Some(Prod::MethodDef);
            let Some(field) = self.collect_field(ast, item) else {
                continue;
            };
            let cf = ClassField {
                field,
                span: ast.span_of(item),
                method,
            };
            match at.get(&field.name) {
                Some(&i) => {
                    log.push(Logger {
                        span: ast.span_of(item),
                        msg: format!("字段 {} 重复声明", self.session.names.resolve(field.name)),
                    });
                    out[i] = cf;
                }
                None => {
                    at.insert(field.name, out.len());
                    out.push(cf);
                }
            }
        }
        out
    }

    /// extendtype 解析出来的类型是不是一个可继承的 class。typedef / 标量 /
    /// 匿名 record / 内建容器（array/table）都不行 —— 名义继承只在 class 之间
    pub(super) fn extends_is_class(&self, ty: TypeId) -> bool {
        match self.session.type_arenas.get_type(ty) {
            Types::Ref { decl, .. } => {
                !decl.is_builtin_container()
                    && self.session.decls.get(decl).kind() == DeclKind::Class
            }
            _ => false,
        }
    }

    /// 从 start 沿 extends 链上溯，绕回 start 就是环。每条闭合的环都在它
    /// 最后声明的那个成员处被查出并掐断（见下），所以从刚填好的这条往上爬
    /// 总会终止；visited 只是防把手 —— 万一有未掐断的环，碰到重复节点就停
    pub(super) fn extends_has_cycle(&self, start: DeclId) -> bool {
        let mut seen: HashSet<DeclId> = HashSet::new();
        let mut cur = start;
        loop {
            let Some(parent) = self
                .session
                .decls
                .get(cur)
                .as_class()
                .and_then(|c| c.extends)
            else {
                return false;
            };
            let Types::Ref { decl, .. } = self.session.type_arenas.get_type(parent) else {
                return false;
            };
            if decl == start {
                return true;
            }
            // 绕回一个见过的、但不是 start 的节点：那是不含 start 的环，不归这条报
            if !seen.insert(decl) {
                return false;
            }
            cur = decl;
        }
    }

    /// `optpub CLASS NAME generics classbody`
    pub(super) fn check_class_decl(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(node_index).children.clone();
        let Some((decl, name)) = self.decl_to_fill(ast, node_index, children[2], "class name", log)
        else {
            return;
        };
        let fields = self.collect_class_fields(ast, children[4], log);
        // 非空体规则：没有父类可继承形状时，空 class 是个没有形状的名义类型，
        // 只能由 `A{}` 造却又造不出任何字段 —— 拒掉。带 extends 的空体合法
        if fields.is_empty() {
            log.push(Logger {
                span: ast.span_of(children[2]),
                msg: format!("class {name} 的类型体不能为空"),
            });
        }
        if let Some(c) = self.session.decls.get_mut(decl).as_class_mut() {
            c.fields = fields;
        }
    }

    /// `optpub CLASS NAME generics ':' extendtype classbody`
    pub(super) fn check_class_decl_extends(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(node_index).children.clone();
        let Some((decl, name)) = self.decl_to_fill(ast, node_index, children[2], "class name", log)
        else {
            return;
        };
        // extendtype 在遍历里已解析成 Ref；不是 class 就不认这条继承
        let extends_ty = self.child_type(children[5]);
        let parent_ok = self.extends_is_class(extends_ty);
        if !parent_ok {
            log.push(Logger {
                span: ast.span_of(children[5]),
                msg: format!("class {name} 只能继承 class"),
            });
        }
        let fields = self.collect_class_fields(ast, children[6], log);
        if let Some(c) = self.session.decls.get_mut(decl).as_class_mut() {
            c.fields = fields;
            c.extends = parent_ok.then_some(extends_ty);
        }
        // 环要在体填完之后查：`class A:B` 先登记时 B 的 extends 还空着，等到
        // 闭合那条（`class B:A`）填上才绕得回来。查到就把这条 extends 掐掉，
        // 免得 lookup_field / assignable 沿链上溯时只能靠深度上限兜底
        if parent_ok && self.extends_has_cycle(decl) {
            log.push(Logger {
                span: ast.span_of(children[2]),
                msg: format!("class {name} 的继承出现环"),
            });
            if let Some(c) = self.session.decls.get_mut(decl).as_class_mut() {
                c.extends = None;
            }
        }
    }

    /// `optpub TYPEDEF NAME generics '=' type`
    pub(super) fn check_type_def(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(node_index).children.clone();
        let Some((decl, _)) = self.decl_to_fill(ast, node_index, children[2], "typedef name", log)
        else {
            return;
        };
        // 目标类型遍历里已解析；递归 typedef 里对自己的引用是 Ref、不展开，所以
        // `typedef Node = {next:Node|nil}` 到这里 target 就是那个 record，不会打转
        let target = self.child_type(children[5]);
        if let Some(t) = self.session.decls.get_mut(decl).as_typedef_mut() {
            t.target = target;
        }
    }

    /// @ClassBody / @FieldDecl：字段的收集在 check_class_decl(_extends) 里一次做完
    /// （要按 DeclId 落库、还要跨字段去重），逐节点钩子无事。字段各自的 `type`
    /// 子树照常在遍历里求值，collect_field 直接读那些结果
    pub(super) fn check_class_body(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) {
    }
    /// 「既标注又带默认值」的字段（`NAME ':' type '=' exp`）：默认值得塞得进标注位，
    /// 和 check_local_decl_init 同一条可赋值性检查。另两种形式没得可查 ——
    /// `NAME ':' type` 无默认值，`NAME '=' exp` 直接拿默认值当类型、谈不上塞不塞得进。
    ///
    /// 后序求值：children[2]（标注）与 children[4]（默认值 exp）此刻都已注册；
    /// 形状判定走 field_decl_shape，和 collect_field_decl 同源
    pub(super) fn check_field_decl(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(node_index).children.clone();
        let (annotated, default) = self.field_decl_shape(ast, node_index);
        if !annotated || !default {
            return;
        }
        let want = self.child_type(children[2]);
        let got = self.child_type(children[4]);
        if !self.session.assignable(got, want) {
            log.push(Logger {
                span: ast.span_of(children[4]),
                msg: format!(
                    "不能把 {} 赋给字段标注的 {} 位",
                    self.session.show(got),
                    self.session.show(want)
                ),
            });
        }
    }

    /// 字段覆盖：class 不允许声明一个父类（含更上层）已经有的同名字段。
    /// 在 finish 里做 —— 那时本文件所有 class 体都填好了，前向继承的父类也在。
    /// 只扫本文件 hoist 的 decl：DeclTable 跨文件累积，别的文件的 class 不重报
    pub(super) fn check_field_overrides(&mut self, log: &mut Vec<Logger>) {
        let decls = self.file_decls.clone();
        for decl in decls {
            // 取父类型和本类自己的字段（名字 + 位置），拷成 owned 后再放开借用，
            // 好接着用 &mut self 的 lookup_field 沿 extends 链查
            let (parent, own) = {
                let Some(class) = self.session.decls.get(decl).as_class() else {
                    continue;
                };
                let Some(parent) = class.extends else {
                    continue;
                };
                let own: Vec<(NameId, Span)> = class
                    .fields
                    .iter()
                    .map(|f| (f.field.name, f.span))
                    .collect();
                (parent, own)
            };
            for (name, span) in own {
                if self.lookup_field(parent, name).is_some() {
                    log.push(Logger {
                        span,
                        msg: format!(
                            "字段 {} 覆盖了继承来的同名字段",
                            self.session.names.resolve(name)
                        ),
                    });
                }
            }
        }
    }

    // ---- 类的方法 ----
    //
    // 类型系统里没有「方法」这个概念，只有字段持有函数值：`a:f(x)` 就是 `a.f(a, x)`，
    // 纯语法糖（README）。所以 methodsig 产出的就是个普通 Func、收进 record 当字段。
    // self 是**显式**写出来的形参、不注入；只是「不标注的 self」其类型默认为本类，
    // 那一步在 resolve_param 里靠 self_default_type 定。
    //
    // 方法和函数变量字段不互为糖：只有方法能被类体外的 `function A:m` /
    // `function A.m` 重定义，也只有方法不参与 `A{…}` 构造。又因为「只给签名」
    // 那条候选式已从文法删掉，方法必带体 —— “声明了却无人定义”这个状态不存在

    /// `classfield : methodsig block END @MethodDef` —— 带内联体的方法。类型仍只看
    /// 签名（children[0]）；体的作用域/流帧在 enter/leave 里按函数体那套开合。
    ///
    /// 没写 `->` 时签名算不出返回列表：methodsig 在文法上先于 block，它求值时
    /// 体里的 return 一条都还没见过。所以推断拖到这里（体已跑完）重拼一份
    pub(super) fn resolve_method_def(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        _log: &mut Vec<Logger>,
    ) -> TypeId {
        let sig_node = ast.get_node(node_index).children[0];
        let sig = self.child_type(sig_node);
        if self.declared_ret(ast, sig_node).is_some() {
            return sig;
        }
        let Types::Func {
            generics, params, ..
        } = self.session.type_arenas.get_type(sig)
        else {
            return sig;
        };
        let ret = self.inferred_ret(node_index);
        self.session.type_arenas.func(generics, params, ret)
    }
    /// `funcname : dotted_name ':' NAME @MethodName` —— 类体外的冒号形式方法名。
    /// 接收者类型就是这个节点的综合属性；紧随其后的 funcbody 从 node_values 读它，
    /// 不再借 TypeLinter 上的一格临时状态跨兄弟节点传值。
    pub(super) fn resolve_method_name(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        let base = ast.get_node(node_index).children[0];
        self.method_receiver_type(ast, base)
    }

    /// `dotted_name : dotted_name '.' NAME @DottedName`。直接的 `A.m` 把 A 的类型作为
    /// 本节点的综合属性；多级路径 `A.b.c` 的根节点返回 UNKNOWN，因此不会把 A 误当
    /// 成最终函数的接收者。点形式不注入 self，这个类型只给显式 self 的默认类型用。
    pub(super) fn resolve_dotted_name(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        _log: &mut Vec<Logger>,
    ) -> TypeId {
        let base = ast.get_node(node_index).children[0];
        if self.get_production(ast, base) == Some(Prod::FuncName) {
            self.method_receiver_type(ast, base)
        } else {
            TypeId::UNKNOWN
        }
    }

    /// funcbody 的直接 funcname 接收者。全局函数声明的形状固定为
    /// `FUNCTION funcname funcbody`：funcname 是已经走完的前一个兄弟节点，所以它的
    /// 接收者 TypeId 此时已在 node_values；点/冒号形式直接由 funcname 的标签区分。
    /// 局部函数和函数表达式没有这种接收者关系。
    pub(super) fn funcbody_receiver(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        funcbody: usize,
    ) -> Option<(TypeId, bool)> {
        let parent = ast.get_node(funcbody).parent?;
        if self.get_production(ast, parent) != Some(Prod::FuncDecl) {
            return None;
        }
        let children = &ast.get_node(parent).children;
        if children.len() != 3 || children.get(2).copied() != Some(funcbody) {
            return None;
        }
        let fname = children[1];
        let colon = match self.get_production(ast, fname) {
            Some(Prod::MethodName) => true,
            Some(Prod::DottedName) => false,
            _ => return None,
        };
        Some((self.child_type(fname), colon))
    }

    /// 类体外的方法定义：`function A:m(…)` 与 `function A.m(self, …)`。
    ///
    /// 只能重定义类体里**声明过的方法**：方法必带体（文法保证），所以类体里
    /// 一定有那条签名可以对；函数变量字段则完全不许用 `function` 去定义 ——
    /// 那是普通变量，只能在构造里给或整体赋值。
    ///
    /// 签名要一致。两种形式的差别只在 self 是不是隐式的（Lua 5.3：只有 ':'
    /// 注入接收者），所以 ':' 形式先在形参表头合成一个本类的 self 再比，'.' 形式
    /// 原样比 —— 后者写没写 self、写成什么类型，都由类体里那条声明说了算（无 self
    /// 的静态方法就这么自然地支持了）。
    ///
    /// 接收者不是类名时直接放过：`function Mod.helper(n)` 是给一张普通表挂函数，
    /// 那是「声明形式给表定形」那一套的事，不归这条管
    pub(super) fn check_external_method(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        fname: usize,
        written: TypeId,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(fname).children.clone();
        let colon = self.get_production(ast, fname) == Some(Prod::MethodName);
        let span = ast.span_of(fname);
        let class = self.method_receiver_type(ast, children[0]);
        if class == TypeId::UNKNOWN {
            return;
        }
        let cname = self.class_name(class);
        let mname = self
            .name_or_panic(ast, children[2], "method name")
            .to_string();
        let name_id = self.session.names.intern(&mname);
        let Some((want, is_method)) = self.lookup_class_field(class, name_id) else {
            log.push(Logger {
                span,
                msg: format!("class {cname} 里没有声明方法 {mname}，类体外只能重定义已声明的方法"),
            });
            return;
        };
        if !is_method {
            log.push(Logger {
                span,
                msg: format!(
                    "{cname}.{mname} 是函数变量字段而不是方法，不能用 function 定义；它只能在构造里给或整体赋值"
                ),
            });
            return;
        }
        let got = if colon {
            self.prepend_self(written, class)
        } else {
            written
        };
        if got != want {
            log.push(Logger {
                span,
                msg: format!(
                    "方法 {mname} 的定义和 class {cname} 里的声明不一致：声明为 {}，这里是 {}",
                    self.session.show(want),
                    self.session.show(got)
                ),
            });
        }
    }

    /// 在函数类型的形参表头插一个 self。`a:m(x)` 是 `a.m(a, x)` 的糖，
    /// 所以 `function A:m(x)` 写出的签名要先补上这个隐式接收者才能和声明对
    pub(super) fn prepend_self(&mut self, func: TypeId, class: TypeId) -> TypeId {
        // methodsig 没有 generics 那格，所以这儿拿到的总是空的；照原样透传，
        // 不假设它一定空，将来泛型方法落地也不用回头改这里
        let Types::Func {
            generics,
            params,
            ret,
        } = self.session.type_arenas.get_type(func)
        else {
            return func;
        };
        let list = self.session.type_arenas.list(params);
        let (mut fixed, vararg) = (list.fixed.clone(), list.vararg);
        fixed.insert(0, class);
        let params = self.session.type_arenas.intern_list(fixed, vararg);
        self.session.type_arenas.func(generics, params, ret)
    }

    /// 方法接收者 `A`（`function A:m`）-> 它的类型。只认单一类名：dotted_name 折成
    /// @FuncName（裸名）时查类型表拿到 class 的 Ref；`a.b:m` 这类带路径的接收者、
    /// 或名字不是 class 的，一律 UNKNOWN —— self 落成 UNKNOWN，体内 self.x 不误报
    pub(super) fn method_receiver_type(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        base: usize,
    ) -> TypeId {
        if self.get_production(ast, base) != Some(Prod::FuncName) {
            return TypeId::UNKNOWN;
        }
        let Some(name) = self.get_child_name(ast, base, 0) else {
            return TypeId::UNKNOWN;
        };
        self.class_ref_by_name(name)
            .map_or(TypeId::UNKNOWN, |(_, class)| class)
    }
    /// 不标注的 `self` 形参该默认成什么类型，`None` = 这个位置没有接收者。
    /// 三种「函数样」祖先各有归属，只认最近的那一层（类体内允许嵌套闭包和
    /// 字段默认值里的函数，它们的 self 不该被当成方法接收者）：
    ///   - methodsig：类体内的方法，接收者就是本类（class_stack 顶）
    ///   - funcbody：只有类体外的**点**形式方法定义算（`function A.m(self, …)`）。
    ///     接收者从该 funcbody 的 funcname 兄弟节点取；嵌套闭包没有这个兄弟关系
    ///   - functype：纯类型位，没有接收者
    pub(super) fn self_default_type(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        param_node: usize,
    ) -> Option<TypeId> {
        let mut cur = ast.get_node(param_node).parent;
        while let Some(p) = cur {
            match self.get_production(ast, p) {
                Some(Prod::MethodSig) => return self.class_stack.last().copied(),
                Some(Prod::FuncBody) => {
                    return self
                        .funcbody_receiver(ast, p)
                        .and_then(|(ty, colon)| (!colon).then_some(ty));
                }
                Some(Prod::FuncType) => return None,
                _ => cur = ast.get_node(p).parent,
            }
        }
        None
    }

    /// class 声明节点 -> 它的 Ref 类型（无实参）。self 默认取本类用它。
    /// 只认顶层抬升过、且确实是 class 的名字；typedef / 标量 / 查不到一律 UNKNOWN
    pub(super) fn class_ref_of(&mut self, ast: &Tree<NodeSyntax<'_>>, decl_node: usize) -> TypeId {
        let Some(name) = self.get_child_name(ast, decl_node, 2) else {
            return TypeId::UNKNOWN;
        };
        self.class_ref_by_name(name)
            .map_or(TypeId::UNKNOWN, |(_, class)| class)
    }
}
