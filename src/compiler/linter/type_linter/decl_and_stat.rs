//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §9 声明与语句 ==================

    /// param : NAME optype —— 形参在这里落进作用域。
    ///
    /// 跑在 @FuncBody 压的那格作用域里：形参表在文法上先于 block，后序遍历到
    /// 这儿时体的 block 还没进，所以形参和泛型形参同层，并且能被体内同名的
    /// `local` 遮蔽 —— 和 Lua 一致
    pub(super) fn resolve_param(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        _log: &mut Vec<Logger>,
    ) -> TypeId {
        let optype = ast.get_node(node_index).children[1];
        // 形参写 self 又不标注：类型默认为接收者那个类（README：self 是显式的
        // 普通形参，只是类型可以省）。标了就按标注；接收者从哪里来、哪些位置
        // 算得上接收者，统一由 self_default_type 定
        let ty = match self.type_annotation(ast, optype) {
            Some(t) => t,
            None if self.get_child_name(ast, node_index, 0) == Some("self") => self
                .self_default_type(ast, node_index)
                .unwrap_or(TypeId::UNKNOWN),
            None => TypeId::UNKNOWN,
        };
        if let Some(name) = self.get_child_name(ast, node_index, 0) {
            // 形参一律算已赋值。调用点少给的实参是 nil，那是调用点的事，
            // 在这儿把形参当成未赋值会让每个函数开头都报一屏
            self.declare_variable(name, ty, ast.span_of(node_index), VarScope::Local, true);
        }
        ty
    }

    /// `local a:number, b` —— 没有初值。标注必须给（README「无初值声明」：
    /// `local a` 不行，没标注就没类型可言）。inited 从哪里来不在这里定：
    /// `new_slot` 里「容得下 nil 的类型声明出来就是诚实的」那一条说了算
    pub(super) fn check_local_decl(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let decl = ast.get_node(node_index).children[1];
        for (name_node, optype) in self.decl_items(ast, decl) {
            let annot = self.type_annotation(ast, optype);
            if annot.is_none() {
                log.push(Logger {
                    span: ast.span_of(name_node),
                    msg: "无初值的 local 必须带类型标注".to_string(),
                });
            }
            let ty = annot.unwrap_or(TypeId::UNKNOWN);
            if let Some(name) = self.get_name(ast, name_node) {
                self.reject_same_scope_redecl(name, ast.span_of(name_node), log);
                self.declare_variable(name, ty, ast.span_of(name_node), VarScope::Local, false);
            }
        }
    }

    /// `local a:number, b = 1, f()` —— 有初值。
    ///
    /// 声明得在后序做：`local n = n` 里右边那个 n 读的是外层的，
    /// 提前到 enter 里声明就会变成自己读自己
    pub(super) fn check_local_decl_init(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = &ast.get_node(node_index).children;
        let values_node = children[3];
        let (decl, values) = (children[1], self.child_type(values_node));
        for (i, (name_node, optype)) in self.decl_items(ast, decl).into_iter().enumerate() {
            let value = self.listnode_value_at(values, i);
            let annot = self.type_annotation(ast, optype);
            self.require_annot_for_empty_table(ast, values_node, i, annot, log);
            // 有标注又有初值：初值得塞得进标注位，可赋值性不过就报。没标注不查 ——
            // 那是把初值的类型直接当声明类型，谈不上「塞不塞得进」
            if let (Some(want), Some(got)) = (annot, value) {
                if !self.session.assignable(got, want) {
                    log.push(Logger {
                        span: ast.span_of(name_node),
                        msg: format!(
                            "不能把 {} 赋给标注的 {} 位",
                            self.session.show(got),
                            self.session.show(want)
                        ),
                    });
                }
            }
            // 标注优先；没标注就拿初值的类型 —— `local e = error` 之后 `e("x")`
            // 能认出 never、从而算作终结，靠的就是这一步
            let ty = annot.or(value).unwrap_or(TypeId::UNKNOWN);
            if let Some(name) = self.get_name(ast, name_node) {
                self.reject_same_scope_redecl(name, ast.span_of(name_node), log);
                // 名字比值多（`local a, b = 1`）：多出来的那些拿到的是 nil，
                // 算不算已赋值交给 new_slot 按类型定
                self.declare_variable(
                    name,
                    ty,
                    ast.span_of(name_node),
                    VarScope::Local,
                    value.is_some(),
                );
            }
        }
    }

    /// 空表字面量推不出形状，所以 `local a = {}` / 顶层 `a = {}` 都得带标注
    /// （README「表的形状一次定死」第一条）。非空字面量自己能定形，不在此列。
    ///
    /// 得按**语法形状**认：类型上字面量 `{}` 和一个已经定形的空 record 是同一个
    /// TypeId，只看类型会把 `local b = a`（a 是 `{}`）也当成没标注的空表报下去
    pub(super) fn require_annot_for_empty_table(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        values: usize,
        i: usize,
        annot: Option<TypeId>,
        log: &mut Vec<Logger>,
    ) {
        if annot.is_some() {
            return;
        }
        let Some(&value) = self.spine(ast, values).get(i) else {
            return;
        };
        if self.get_production(ast, value) != Some(Prod::TableEmpty) {
            return;
        }
        log.push(Logger {
            span: ast.span_of(value),
            msg: "空表 `{}` 推不出形状，必须带类型标注".to_string(),
        });
    }

    pub(super) fn check_assign(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let node = ast.get_node(node_index);
        let values_node = node.children[2];
        let (targets, values) = (node.children[0], self.child_type(values_node));
        for (i, var) in self.spine(ast, targets).into_iter().enumerate() {
            // var : prefixexp optype。只有 prefixexp 恰好是裸名字时才谈得上声明；
            // `t.k = v` / `t[i] = v` 只是写字段，那个 `t` 已经当成读查过了
            let children = &ast.get_node(var).children;
            let (prefix, optype) = (children[0], children[1]);
            let got = self.listnode_value_at(values, i);
            if self.get_production(ast, prefix) != Some(Prod::VarRef) {
                self.check_write_target(ast, var, got, log);
                continue;
            }
            let Some(name) = self.get_child_name(ast, prefix, 0) else {
                continue;
            };
            if let Some(slot) = self.lookup_declared(name) {
                // 赋值：inited 置上。声明处的类型是唯一权威，赋进来的值得塞得进它 ——
                // 再标注一次也只是又走这条，冲突的标注会体现在值的类型上
                self.mark_inited(name);
                if let Some(got) = got {
                    if !self.session.assignable(got, slot.ty) {
                        log.push(Logger {
                            span: ast.span_of(prefix),
                            msg: format!(
                                "不能把 {} 赋给 {}（声明为 {}）",
                                self.session.show(got),
                                name,
                                self.session.show(slot.ty)
                            ),
                        });
                    }
                }
                continue;
            }
            // 没声明过：这里就是全局的声明点，而声明点只认顶层
            if !self.is_chunk_top(ast, node_index) {
                log.push(Logger {
                    span: ast.span_of(prefix),
                    msg: format!("未声明的变量 {name}；全局变量的声明点只能在文件顶层"),
                });
            }
            // 声明点上的空表和 local 同一条规矩：裸 `a = {}` 也推不出形状
            let annot = self.type_annotation(ast, optype);
            self.require_annot_for_empty_table(ast, values_node, i, annot, log);
            // 报完照样登记：不登的话同一个名字后面每出现一次就再报一道
            let ty = annot.or(got).unwrap_or(TypeId::UNKNOWN);
            self.declare_global(name, ty, ast.span_of(prefix), VarScope::Global, true);
        }
    }

    /// `var : prefixexp optype` 里 prefixexp 不是裸名字的那些写位。三件事：
    ///   - 标注只许贴在裸名字上：`a.b:T = v` / `t[i]:T = v` 都报。标注是**声明**
    ///     的一部分，而字段的声明在 class 体 / record 类型里，写位无权开新字段
    ///   - 赋值目标不能是函数调用或括号表达式：文法把它们也收进了 prefixexp，
    ///     但语义上它们不是左值
    ///   - 字段写本身交给 `check_field_write`；`t[k] = v` 只核值 —— 键的核对在
    ///     `resolve_index` 里做过了，而节点类型就是它算出的元素类型
    pub(super) fn check_write_target(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        var: usize,
        got: Option<TypeId>,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(var).children.clone();
        let (prefix, optype) = (children[0], children[1]);
        let span = ast.span_of(prefix);
        if self.type_annotation(ast, optype).is_some() {
            log.push(Logger {
                span,
                msg: "类型标注只能贴在裸变量名上：字段和元素的类型由它所属的 class / record / 容器给出"
                    .to_string(),
            });
        }
        match self.get_production(ast, prefix) {
            Some(Prod::Dot) => self.check_field_write(ast, prefix, got, log),
            Some(Prod::Index) => {
                // 不是容器时 resolve_index 给的是 ANY，assignable 自然放过
                if let Some(got) = got {
                    let want = self.child_type(prefix);
                    self.expect_assignable(got, want, span, "元素", log);
                }
            }
            Some(Prod::Call | Prod::MethodCall | Prod::Paren | Prod::TurboFish) => {
                log.push(Logger {
                    span,
                    msg: "赋值目标只能是变量、字段或表元素".to_string(),
                });
            }
            _ => {}
        }
    }

    /// `t.k = v` 里 `t.k` 那一侧。四件事：
    ///   - 类名当基（`A.m = f`）：类名只在类型命名空间里，它不是一个变量。
    ///     方法要改就写 `function A:m(…)`，字段是实例上的东西
    ///   - 方法字段被整体覆盖（`a.m = f`）：方法只能在类体里定义、类体外用
    ///     `function A:m` 重定义，赋值这条路不开 —— 开了就等于方法可以每个实例被
    ///     换成另一个函数，那就是函数变量字段而不是方法
    ///   - 字段必须已经存在：表和 class 的形状一次定死，运行时不能长新字段。
    ///     `table<K,V>` 不在此列 —— 它的形状就是「任意个 K 键」，写不出新形状
    ///   - 值得塞得进字段的类型
    pub(super) fn check_field_write(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        prefix: usize,
        got: Option<TypeId>,
        log: &mut Vec<Logger>,
    ) {
        // `prefixexp '.' NAME`：基在 0、字段名在 2
        let dot = ast.get_node(prefix).children.clone();
        let span = ast.span_of(prefix);
        let Some(fname) = self.get_name(ast, dot[2]) else {
            return;
        };
        let fname = fname.to_string();
        if self.bare_class_ref(ast, dot[0]).is_some() {
            let cname = self
                .get_child_name(ast, dot[0], 0)
                .unwrap_or("")
                .to_string();
            log.push(Logger {
                span,
                msg: format!(
                    "{cname} 是类名而不是变量：方法请写 function {cname}:{fname}(…) 重定义，字段是实例上的东西"
                ),
            });
            return;
        }
        let owner = self.child_type(dot[0]);
        let id = self.session.names.intern(&fname);
        if let Some((_, true)) = self.lookup_class_field(owner, id) {
            log.push(Logger {
                span,
                msg: format!(
                    "{fname} 是方法，不能赋值覆盖；要一个能每个实例不同的函数成员就把它声明成 {fname} : function(…)"
                ),
            });
            return;
        }
        // 已有字段：只核值。字段类型是声明处给的，写不改它
        if let Some(want) = self.lookup_field(owner, id) {
            if let Some(got) = got {
                self.expect_assignable(got, want, span, &format!("字段 {fname}"), log);
            }
            return;
        }
        // 容器按键写：`t.k` 就是键为 "k" 那一项，所以键得是 string
        let shape = self.resolve_alias(owner);
        if let Some((k, v)) = self.session.type_arenas.as_map(shape) {
            self.expect_key(TypeId::STRING, k, span, log, "表键");
            if let Some(got) = got {
                self.expect_assignable(got, v, span, "表值", log);
            }
            return;
        }
        // 拿不准形状的不报；形状确定的（record / class / 交类型）才该报
        if !self.is_known_shape(shape) {
            return;
        }
        log.push(Logger {
            span,
            msg: format!(
                "{} 上没有字段 {fname}：形状一次定死，运行时不能新增字段",
                self.session.show(owner)
            ),
        });
    }

    /// `extern NAME ':' type` —— 宿主注入的全局。
    ///
    /// 不核实它真的存不存在（那是宿主的事），所以声明处就算已赋值；
    /// 「只许顶层」文法里表达不了（参见 table.rs 里这条产生式的注释），归这里拦
    pub(super) fn check_extern(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let ty = self.pass_through(ast, node_index, 3);
        let span = ast.span_of(node_index);
        let Some(name) = self.get_child_name(ast, node_index, 1) else {
            return;
        };
        if !self.is_chunk_top(ast, node_index) {
            log.push(Logger {
                span,
                msg: "extern 只能写在文件顶层".to_string(),
            });
        }
        if !self.declare_global(name, ty, span, VarScope::Extern, true) {
            log.push(Logger {
                span,
                msg: format!("{name} 已经声明过了"),
            });
        }
    }

    /// 两种形状：`LOCAL FUNCTION NAME funcbody`（四个孩子）声明局部，
    /// `FUNCTION funcname funcbody`（三个）写的是全局。
    ///
    /// 局部那种的名字已经在 enter 里先声明过了（`local function f` 等于
    /// `local f; f = function...`，不先声明就递归不了），这里只是用真类型覆一道。
    ///
    /// funcname 是 @DottedName / @MethodName 时不管声明：那是给已有的表或类挂函数。
    /// 接收者是类名的那一支要校方法签名，转给 check_external_method
    pub(super) fn check_func_decl(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = &ast.get_node(node_index).children;
        if children.len() == 4 {
            let ty = self.child_type(children[3]);
            if let Some(name) = self.get_child_name(ast, node_index, 2) {
                self.declare_variable(name, ty, ast.span_of(node_index), VarScope::Local, true);
            }
            return;
        }
        let (fname, ty) = (children[1], self.child_type(children[2]));
        match self.get_production(ast, fname) {
            Some(Prod::FuncName) => {}
            // `function A:m` / `function A.m`：类体外重定义类里声明的方法
            Some(Prod::MethodName | Prod::DottedName) => {
                self.check_external_method(ast, fname, ty, log);
                return;
            }
            _ => return,
        }
        let span = ast.span_of(fname);
        let Some(name) = self.get_child_name(ast, fname, 0) else {
            return;
        };
        if self.is_chunk_top(ast, node_index) {
            // prepare 里抬升时已经登记过名字，这里只补类型、并把 inited 置上：
            // 声明语句真的执行到了，从这句起立即位置也读得了
            self.session.patch_hoisted_type(name, ty);
            self.mark_inited(name);
            return;
        }
        log.push(Logger {
            span,
            msg: format!(
                "未声明的变量 {name}；`function {name}` 写的是全局变量，声明点只能在文件顶层"
            ),
        });
        self.declare_global(name, ty, span, VarScope::Global, true);
    }

    // ---- 驱动点要用的几个动作 ----

    /// chunk 的直接语句列表。根就是 chunk 的 @Block（`chunk : block` 被折叠了）；
    /// children[0] 可能是 stat_list，也可能是 retstat（`block : retstat`）—— 后者 spine
    /// 返回它自己，各调用点那层形状判断自然会跳过。
    /// 三趟 hoist 都只浅扫这一层：嵌在 if / do 里的声明不是顶层声明点
    pub(super) fn top_level_stats(&self, ast: &Tree<NodeSyntax<'_>>) -> Vec<usize> {
        let Some(root) = ast.get_root() else {
            return Vec::new();
        };
        let Some(&stats) = ast.get_node(root).children.first() else {
            return Vec::new();
        };
        self.spine(ast, stats)
    }

    /// 顶层类型声明的三条产生式 -> 「是不是 class」，都不是就 None。
    /// 三条形状都是 `optpub CLASS/TYPEDEF NAME …`，NAME 恒在 children[2]
    pub(super) fn type_decl_is_class(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        stat: usize,
    ) -> Option<bool> {
        match self.get_production(ast, stat) {
            Some(Prod::ClassDecl | Prod::ClassDeclExtends) => Some(true),
            Some(Prod::TypeDef) => Some(false),
            _ => None,
        }
    }

    /// 顶层函数抬升：先把名字登记上，类型留到 @FuncDecl 的 leave 补。
    ///
    /// 为什么要抬升：`function a() b() end  function b() end` 在 Lua 里合法 ——
    /// b 的声明语句在后面，但 a 的体要等被调用时才求值。inited 从 false 起步，
    /// 于是顶层立即位置读它照样会报而函数体内不查 —— 抬升和定值分析共用同一位
    /// （见 VarScope::HoistedFn 的注释）
    pub(super) fn hoist_top_level(&mut self, ast: &Tree<NodeSyntax<'_>>) {
        for stat in self.top_level_stats(ast) {
            let children = &ast.get_node(stat).children;
            // 只认 `FUNCTION funcname funcbody`（3 个孩子）里 funcname 是裸名的：
            // `local function` 不是全局，`a.b` / `a:b` 是给已有的表挂函数
            if !matches!(
                ast.get_node(stat).get_data(),
                NodeSyntax::Prod(Some(Prod::FuncDecl))
            ) || children.len() != 3
            {
                continue;
            }
            let fname = children[1];
            if !matches!(
                ast.get_node(fname).get_data(),
                NodeSyntax::Prod(Some(Prod::FuncName))
            ) {
                continue;
            }
            let span = ast.span_of(fname);
            if let Some(name) = self.get_child_name(ast, fname, 0) {
                // 类型先留 UNKNOWN；重名（两条 `function f`）时 declare_global 返回
                // false，不管 —— 后一条在 Lua 里就是一次重新赋值
                self.declare_global(name, TypeId::UNKNOWN, span, VarScope::HoistedFn, false);
                // 再把 inited 压回 false。`new_slot` 会因为 `admits_nil(UNKNOWN)`
                // 把它当成已赋值，那一条放过是为了「别在推断失败之上再叠一条误报」；
                // 可这里不是推断失败 —— 声明语句确实还没执行到，那一格此刻真的是 nil，
                // `a() function a() end` 在 Lua 里就是 attempt to call a nil value
                self.session.set_global_inited(name, false);
            }
        }
    }

    /// 顶层 class / typedef 名字预登记：两阶段注册的第一步（P0a）。
    ///
    /// 先把名字占上（此刻体是空的：class 无字段、typedef 目标是 UNKNOWN），拿到
    /// DeclId 并 `declare_type` 进文件级作用域。三种引用因此都解析成同一个 DeclId：
    /// 体里对自己的引用（`typedef Node = {next:Node|nil}`）、互相引用的两个 class、
    /// 写在声明语句之前的前向引用。填体（字段 / extends / target）是
    /// check_class_decl / check_class_decl_extends / check_type_def 的事。
    ///
    /// 只认顶层（见 top_level_stats）：嵌在 if / do 里的 class 不是顶层声明点，
    /// 那种情形归各自的 check_* 去拦。
    /// 非 pub 的名字只落在文件级作用域（跨文件导出是 P5 的 Session::export）
    pub(super) fn hoist_top_level_types(&mut self, ast: &Tree<NodeSyntax<'_>>) {
        for stat in self.top_level_stats(ast) {
            let Some(is_class) = self.type_decl_is_class(ast, stat) else {
                continue;
            };
            let Some(&name_node) = ast.get_node(stat).children.get(2) else {
                continue;
            };
            let Some(name) = self.get_name(ast, name_node) else {
                continue;
            };
            let span = ast.span_of(name_node);
            let name_id = self.session.names.intern(name);
            let decl = if is_class {
                self.session.decls.declare_class(name_id, span)
            } else {
                self.session.decls.declare_typedef(name_id, span)
            };
            // 同一文件里两条同名声明：declare_type 返回 false，重名诊断留给
            // check_class_decl / check_type_def（那里才知道是 class 撞 typedef 还是别的）
            self.declare_type(name, TypeRef::Decl(decl));
            // 同时登记进模块级的 module_types（不管 pub 与否）：别的文件 import 这个
            // 名字但查不到导出时，靠它分辨「没 pub」与「根本没这个类型」
            self.session
                .declare_module_type(self.current_module, name_id, decl);
            // 本文件的这几条留个底：字段覆盖的收口要在 finish 里回头扫它们
            self.file_decls.push(decl);
            // 泛型形参在这里一次性铸好身份、押进 decl.generics：实例化处（arity 校验、
            // subst_ref_args 的代入）在任何体被遍历之前就得能问到，所以放 P0a
            self.hoist_decl_generics(ast, decl, stat);
        }
    }

    /// `generics` 那一格里写出的形参名（按序，带 span）。空 generics 折成零孩子的
    /// `Prod(None)`，非空才是 `@Generics : '<' typeparams '>'` —— typeparams 在
    /// children[1]，单个折成裸 @TypeParam、多个是 @ListTail 链，spine 两者都收得住；
    /// @TypeParam / @TypeParamBound 的名字都在 children[0]。
    /// 上界 `<T : Cmp>` 不在这儿看：它由 resolve_type_param_bound 在遍历到
    /// @TypeParamBound 时填进 GenericInfo.constraint
    pub(super) fn generic_param_names<'t>(
        &self,
        ast: &'t Tree<NodeSyntax<'t>>,
        generics_node: usize,
    ) -> Vec<(&'t str, Span)> {
        if self.get_production(ast, generics_node) != Some(Prod::Generics) {
            return Vec::new();
        }
        let list = ast.get_node(generics_node).children[1];
        self.spine(ast, list)
            .into_iter()
            .filter_map(|tp| {
                self.get_child_name(ast, tp, 0)
                    .map(|nm| (nm, ast.span_of(tp)))
            })
            .collect()
    }

    /// 把 `stat`（class / typedef 声明）的泛型形参铸成 GenericId 押进 `decl.generics`。
    /// 只铸身份、不进作用域：名字挂进 type_names 是 scope_decl_generics 的事（体遍历时
    /// 才需要），这里赶在所有体之前把身份和个数定死，好让别处的 `Box<..>` 校 arity
    pub(super) fn hoist_decl_generics(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        decl: DeclId,
        stat: usize,
    ) {
        // generics 恒在 class / typedef 的 children[3]
        let Some(&generics_node) = ast.get_node(stat).children.get(3) else {
            return;
        };
        let mut gids = Vec::new();
        for (nm, span) in self.generic_param_names(ast, generics_node) {
            let name_id = self.session.names.intern(nm);
            gids.push(self.session.decls.fresh_generic(name_id, span));
        }
        self.session.decls.get_mut(decl).generics = gids;
    }

    /// 把 `decl_node` 这条 class/typedef 的泛型形参声明进**当前**作用域（name -> Generic），
    /// 于是体里的 `T` 查 type_names 查得到、解析成 Types::Generic。形参身份 P0a 已铸好，
    /// 这里只按名字挂上、不再 fresh（否则每趟遍历都会重铸、身份对不上）。P0a 没抬升的
    /// （嵌套声明）拿不到 decl，直接跳过 —— 名义类型只认顶层
    pub(super) fn scope_decl_generics(&mut self, ast: &Tree<NodeSyntax<'_>>, decl_node: usize) {
        let Some(name) = self.get_child_name(ast, decl_node, 2) else {
            return;
        };
        let Some(decl) = self.hoisted_decl(ast, decl_node, name) else {
            return;
        };
        for gid in self.session.decls.get(decl).generics.clone() {
            let s = self
                .session
                .names
                .resolve(self.session.decls.generic(gid).name())
                .to_string();
            self.declare_type(&s, TypeRef::Generic(gid));
        }
    }

    /// 把 funcbody 的泛型形参声明进**当前**（funcbody 那格）作用域，于是形参标注 /
    /// 返回类型 / 体里的 `T` 都解析成 Types::Generic。和 class/typedef 不同：函数不是
    /// 名义声明、没有 DeclId 存形参身份，所以每次进体现铸现挂 —— 每个 funcbody 主遍历
    /// 只进一次，GenericId 也只是个匿名标识，不必像 class 那样在 P0a 预铸再复用
    pub(super) fn scope_func_generics(&mut self, ast: &Tree<NodeSyntax<'_>>, funcbody: usize) {
        // generics 恒在 funcbody 的 children[0]
        let generics_node = ast.get_node(funcbody).children[0];
        for (nm, span) in self.generic_param_names(ast, generics_node) {
            let name_id = self.session.names.intern(nm);
            let gid = self.session.decls.fresh_generic(name_id, span);
            self.declare_type(nm, TypeRef::Generic(gid));
        }
    }

    /// prepare 第三趟：把顶层 class / typedef 的**体**提前填好。P1 的 check_* 本在
    /// 各自 leave 里填体，但那时机在方法体之后 —— 内联方法体里的 `self.x` 就
    /// 查不到本类字段。这里赶在主遍历之前，先把签名子树求值、把字段/父类/别名
    /// 目标灌进 ClassInfo，于是 lookup_field 在整个主遍历里都读得到。
    ///
    /// 只求**签名**、不进任何函数体：体依赖 enter 铺的流帧/作用域，这趟没有，且
    /// 体要留给主遍历正经检查。诊断也不在这发（用 sink 吞掉）—— 重名/空体/继承环
    /// 仍由主遍历里的 check_* 报，避免报两遍。node_values 是主遍历的地盘，这趟只借
    /// 它中转类型，末尾清空交还给主遍历重算
    pub(super) fn hoist_class_bodies(&mut self, ast: &Tree<NodeSyntax<'_>>) {
        // 这趟不发诊断：check_* 里的重名/空体/继承环都报进这个 sink 然后丢掉，
        // 真正的那一份由主遍历的 leave 报，不能重复
        let mut sink: Vec<Logger> = Vec::new();
        for stat in self.top_level_stats(ast) {
            let Some(is_class) = self.type_decl_is_class(ast, stat) else {
                continue;
            };
            // 类体内不标注的 self 默认取本类：进体先把 Ref 押上（typedef 不需要）
            if is_class {
                let ty = self.class_ref_of(ast, stat);
                self.class_stack.push(ty);
            }
            // 形参（含字段默认值闭包的形参）求签名时会 declare，收进这格用完即弃
            self.scope_stack.push(Scope::new());
            // 体里的 `T` 要解成 Generic，泛型形参得先挂进这格（P0a 已铸好身份）
            self.scope_decl_generics(ast, stat);
            self.eager_resolve_sig(ast, stat, &mut sink);
            self.scope_stack.pop();
            if is_class {
                self.class_stack.pop();
            }
        }
        // node_values 是主遍历的地盘：这趟只借它把类型中转给 collect_*，清掉重来，
        // 否则主遍历重求这些子树时 register_value 会撞重入
        self.node_values.clear();
    }

    /// 按后序把一棵子树求值，但**不进函数体**（MethodDef / funcbody 的 block）：
    /// 只为让 collect_class_fields / check_type_def 读得到签名类型。体里的 return /
    /// 局部声明依赖主遍历在 enter 铺好的流帧与作用域，这趟不具备；字段默认值里
    /// 的闭包体同理，只取它的签名、体留给主遍历
    /// prepare 第四趟把顶层全局函数的**签名**提前算出来、补给抬升过的那个名字。
    ///
    /// hoist_top_level 只占名字（类型 UNKNOWN），真类型要等 check_func_decl 在它自己的
    /// leave 里补 —— 而那已经在体之后，于是体内的递归调用、以及写在声明语句之前的
    /// 前向调用都只能拿到 UNKNOWN，实参根本没人核。这一趟赶在主遍历之前把签名灌进
    /// 那个全局位，README：「签名在进入函数体之前就已登记，递归调用的实参照常检查」。
    ///
    /// 和 hoist_class_bodies 一个路子：只求签名不进体、诊断扔进 sink、末尾清空
    /// node_values 交还主遍历重算。`local function f` 不在此列：它不抬升，
    /// 声明点就在主遍历里，提前求它的签名会和主遍历撞重入
    pub(super) fn hoist_func_signatures(&mut self, ast: &Tree<NodeSyntax<'_>>) {
        let mut sink: Vec<Logger> = Vec::new();
        for stat in self.top_level_stats(ast) {
            let children = ast.get_node(stat).children.clone();
            if self.get_production(ast, stat) != Some(Prod::FuncDecl) || children.len() != 3 {
                continue;
            }
            // 只认裸名字：`a.b` / `a:b` 是给已有的表或类挂函数，不占全局位
            if self.get_production(ast, children[1]) != Some(Prod::FuncName) {
                continue;
            }
            let Some(name) = self.get_child_name(ast, children[1], 0).map(str::to_string) else {
                continue;
            };
            // 形参与泛型形参求签名时会 declare，收进这格用完即弃
            self.scope_stack.push(Scope::new());
            // 泛型形参得先挂进这格：eager 那趟不走 enter，`scope_func_generics`
            // 没人替它调，`function map<T,U>(t:array<T>, …)` 的 T 就会解析成
            // unknown，抬升上去的全局签名从此既认不出形参也核不对实参
            self.scope_func_generics(ast, children[2]);
            self.eager_resolve_sig(ast, children[2], &mut sink);
            let ty = self.child_type(children[2]);
            self.scope_stack.pop();
            self.session.patch_hoisted_type(&name, ty);
        }
        // node_values 是主遍历的地盘：这趟只借它中转类型，清掉重来
        self.node_values.clear();
    }

    /// 按后序把一棵子树求值，但**不进函数体**（MethodDef / funcbody 的 block）：
    /// 只为让 collect_class_fields / check_type_def 读得到签名类型。体里的 return /
    /// 局部声明依赖主遍历在 enter 铺好的流帧与作用域，这趟不具备；字段默认值里
    /// 的闭包体同理，只取它的签名、体留给主遍历
    pub(super) fn eager_resolve_sig(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
        log: &mut Vec<Logger>,
    ) {
        let node_prod = self.get_production(ast, node);
        for c in ast.get_node(node).children.clone() {
            if matches!(node_prod, Some(Prod::MethodDef | Prod::FuncBody))
                && self.get_production(ast, c) == Some(Prod::Block)
            {
                continue;
            }
            self.eager_resolve_sig(ast, c, log);
        }
        self.resolve(ast, node, log);
    }

    /// 分支入口处的 nil 收窄。`local s:string|nil` 之后 `if s ~= nil then print(#s) end`
    /// 能过，就靠这一步：把「去掉 nil 之后的类型」记进本块刚压好那格作用域的
    /// `narrowed` 里。出块弹作用域，收窄自然失效 —— 不必另建一套回滚。
    ///
    /// 只认裸名字的测试（`s`、`s ~= nil`、`nil == s`）。字段路径（`a.b ~= nil`）
    /// 不收：那得按路径整个认同，而中间任何一段被写过就得作废，
    /// 不是这一层让得起的代价
    pub(super) fn bind_narrowing(&mut self, ast: &Tree<NodeSyntax<'_>>, block: usize) {
        let Some((cond, then_branch)) = self.branch_guard(ast, block) else {
            return;
        };
        let Some((name, test)) = self.nil_test(ast, cond) else {
            return;
        };
        // 这一支跑得到，就说明那个名字不是 nil。`if s then` 反面无效：
        // 落空那一支里它可能是 false 而不是 nil
        let drops_nil = match test {
            NilTest::NotNil | NilTest::Truthy => then_branch,
            NilTest::IsNil => !then_branch,
        };
        if !drops_nil {
            return;
        }
        let Some(slot) = self.lookup_variable(&name) else {
            return;
        };
        let Some(ty) = self.without_nil(slot.ty) else {
            return;
        };
        if let Some(scope) = self.scope_stack.last_mut() {
            scope.narrowed.insert(name, ty);
        }
    }

    /// 本块是哪条分支：`(条件节点, 是不是条件为真那一支)`。
    /// 不带条件的块（`do` / 函数体 / for 体）给 `None`
    pub(super) fn branch_guard(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        block: usize,
    ) -> Option<(usize, bool)> {
        let parent = ast.get_node(block).parent?;
        let children = &ast.get_node(parent).children;
        match self.get_production(ast, parent) {
            // IF exp THEN block elseif_list END
            Some(Prod::If) => Some((children[1], true)),
            // IF exp THEN block elseif_list ELSE block END
            Some(Prod::IfElse) => Some((children[1], block == children[3])),
            // elseif_list ELSEIF exp THEN block
            Some(Prod::ElseIf) => Some((children[2], true)),
            // WHILE exp DO block END —— 体内条件为真，和 if 同理
            Some(Prod::While) => Some((children[1], true)),
            _ => None,
        }
    }

    /// 条件里的 nil 测试：`(被测的名字, 哪一种)`。
    /// `and` 串起来的多个测试不拆 —— 只开最直接的那一口
    pub(super) fn nil_test(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        cond: usize,
    ) -> Option<(String, NilTest)> {
        // 裸名字当条件：`if s then`
        if self.get_production(ast, cond) == Some(Prod::VarRef) {
            let name = self.get_name(ast, ast.get_node(cond).children[0])?;
            return Some((name.to_string(), NilTest::Truthy));
        }
        if self.get_production(ast, cond) != Some(Prod::BinOp) {
            return None;
        }
        let children = &ast.get_node(cond).children;
        let test = match ast.get_node(children[1]).get_data() {
            NodeSyntax::Token(Token::OPERATOR(OpType::NE)) => NilTest::NotNil,
            NodeSyntax::Token(Token::OPERATOR(OpType::EQ)) => NilTest::IsNil,
            _ => return None,
        };
        // 两边都试：`s ~= nil` 和 `nil ~= s` 是一回事
        for (var, lit) in [(children[0], children[2]), (children[2], children[0])] {
            let is_nil = matches!(
                ast.get_node(lit).get_data(),
                NodeSyntax::Token(Token::RESERVED(Reserved::NIL))
            );
            if is_nil && self.get_production(ast, var) == Some(Prod::VarRef) {
                let name = self.get_name(ast, ast.get_node(var).children[0])?;
                return Some((name.to_string(), test));
            }
        }
        None
    }

    /// 去掉 nil 之后的类型。`None` = 这个类型本来就不含 nil（没必要收窄），
    /// 或者它并不是一个分得开的联类型（any / unknown 那两个口子无处下刀）。
    /// 别名先展开：`typedef Opt = string|nil` 同样收得了，代价是收窄后
    /// 得到的是展开形而不是那个别名
    pub(super) fn without_nil(&mut self, ty: TypeId) -> Option<TypeId> {
        let shape = self.resolve_alias(ty);
        let Types::Union(l) = self.session.type_arenas.get_type(shape) else {
            return None;
        };
        let variants = self.session.type_arenas.list(l).fixed.clone();
        if !variants.contains(&TypeId::NIL) {
            return None;
        }
        let rest: Vec<TypeId> = variants.into_iter().filter(|&v| v != TypeId::NIL).collect();
        Some(self.session.type_arenas.union(rest))
    }

    /// for 的控制变量。它们在文法上是 for 语句的孩子，可见范围却是循环体，
    /// 所以在体的 block 刚压好作用域时声明。那时标注（在 block 之前）已经算好了
    pub(super) fn bind_loop_vars(&mut self, ast: &Tree<NodeSyntax<'_>>, block: usize) {
        let Some(parent) = ast.get_node(block).parent else {
            return;
        };
        let children = &ast.get_node(parent).children;
        match ast.get_node(parent).get_data() {
            // FOR NAME optype '=' exp ',' exp [',' exp] DO block END
            NodeSyntax::Prod(Some(Prod::ForNum | Prod::ForNumStep)) => {
                // 数值 for 的控制变量必然是 number，标注只是写着顺手
                let ty = self
                    .type_annotation(ast, children[2])
                    .unwrap_or(TypeId::NUMBER);
                let span = ast.span_of(children[1]);
                if let Some(name) = self.get_child_name(ast, parent, 1) {
                    self.declare_variable(name, ty, span, VarScope::Local, true);
                }
            }
            // FOR decllist IN explist DO block END。迭代器给什么类型还算不出来，
            // 没标注就是 UNKNOWN；控制变量一律算已赋值
            NodeSyntax::Prod(Some(Prod::ForIn)) => {
                for (name_node, optype) in self.decl_items(ast, children[1]) {
                    let ty = self.type_annotation(ast, optype).unwrap_or(TypeId::UNKNOWN);
                    if let Some(name) = self.get_name(ast, name_node) {
                        let span = ast.span_of(name_node);
                        self.declare_variable(name, ty, span, VarScope::Local, true);
                    }
                }
            }
            _ => {}
        }
    }

    /// @VarRef 的两条诊断。为何在前序：看 resolve_var_ref 的注释
    pub(super) fn check_var_read(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
        log: &mut Vec<Logger>,
    ) {
        // 写位既可能是声明点又可能是赋值点，两条都不适用，归 check_assign
        if self.is_writing(ast, node) {
            return;
        }
        let span = ast.span_of(node);
        let Some(name) = self.get_child_name(ast, node, 0) else {
            return;
        };
        // 体里读自己的名字：没标返回类型的话这一读就让推断依赖自己。
        // 报不报和名字查得到查不到无关，所以在查表之前
        self.check_recursive_ret(name, span, log);
        let Some((scope, slot)) = self.lookup_variable_at(name) else {
            // 类名不在变量表里（hoist_top_level_types 只登记类型名），但它确实能
            // 写在值位：`A{…}` 构造、`A.m(…)` 静态方法。那两条各自有钩子，
            // 这里不该再按「未声明的变量」报一道
            if self.class_ref_by_name(name).is_some() {
                return;
            }
            log.push(Logger {
                span,
                msg: format!("未声明的变量 {name}"),
            });
            return;
        };
        if !slot.inited && self.init_checkable(scope) {
            log.push(Logger {
                span,
                msg: format!("变量 {name} 可能尚未赋值"),
            });
        }
    }

    /// 不可达语句。上一条语句把当前帧终结掉了（return / break / goto /
    /// 调了个不返回的函数），它后面的都到不了。一个帧只报第一条：
    /// 死代码是成段的，逐条报就是一屏
    pub(super) fn check_reachable(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
        log: &mut Vec<Logger>,
    ) {
        if !self.flow_terminated() || !self.follows_stat(ast, node) {
            return;
        }
        let Some(frame) = self.flow_stack.last_mut() else {
            return;
        };
        if frame.dead_reported {
            return;
        }
        frame.dead_reported = true;
        log.push(Logger {
            span: ast.span_of(node),
            msg: "这条语句到不了：上一条已经 return / break / goto，或者调了个不返回的函数"
                .to_string(),
        });
    }

    // ---- 语句钩子 ----
    //
    // 这五个在 §5 的分派表里各占一行，但真正的活都在驱动点的 enter/leave 里干
    // （作用域进出、流帧开合、终结与降级）。留成空壳而不删，是为了让分派表
    // 一个标签一行地对得齐，读表就知道每条语句归谁管；真要收口的诊断各自
    // 在原处标了「见 …」

    /// @Block：作用域进出、分支合并都在 enter/leave 里。这里无事
    pub(super) fn check_block(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) {
    }
    /// 带分支的语句（if / 循环 / do）：流帧按 `join_mode` 在驱动点配对开合
    pub(super) fn check_control_flow(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) {
    }
    /// @ExprStat：「调了不返回的函数」这条终结在 leave 里问 `is_noreturn_stat`
    pub(super) fn check_expr_stat(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) {
    }
    /// @Goto / @Label：降级（`flow_untrust`）与终结 / 救活都在 enter/leave 里
    pub(super) fn check_jump(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) {
    }

    // ---- return 与返回类型 ----
    //
    // 两个方向共用栈顶那格 `RetFrame`：标了 `->` 就拿 return 去核对它，
    // 没标就反过来—— 把 return 攒起来，出体时合成函数类型的返回列表。
    // 于是自引用会让推断依赖自己，那一条由 `check_recursive_ret` 拦

    /// @Return / @ReturnVoid。终结当前帧在 leave 里，这里只管值：
    /// 标了 `->` 就逐位核对，没标就攒给 `inferred_ret` 去推。
    /// 顶层 chunk 的 `return` 在 Lua 里合法，而它没签名可核，直接跳过
    pub(super) fn check_return(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        // @Return 是 `RETURN explist [';']`，@ReturnVoid 根本没值那一格
        let got = match self.get_production(ast, node_index) {
            Some(Prod::Return) => self.pass_through(ast, node_index, 1),
            _ => TypeId::VOID,
        };
        let Some(&RetFrame { sig, annotated, .. }) = self.ret_stack.last() else {
            return;
        };
        if !annotated {
            self.ret_stack.last_mut().unwrap().seen.push(got);
            return;
        }
        let want = self.declared_ret(ast, sig).unwrap_or(TypeId::VOID);
        // `-> never` 说的是「这个函数不返回」，不是一个返回值位子，
        // 逐位核对对它无意（`return` 不算「少给了一个 never」）
        if want == TypeId::NEVER {
            return;
        }
        let span = ast.span_of(node_index);
        let (items, spread) = self.value_items(got, span);
        let want = self.session.type_arenas.as_list(want);
        let want = self.session.type_arenas.list(want).clone();
        self.check_positional(&items, &want, spread, span, "返回值", log);
    }

    /// 一个多值摊成逐位的 `(报错落点, 类型)`，外加一位「末位摊不摊得开」。
    /// 落点只有一个（整条 return 语句），实参那边才逐个有自己的节点
    pub(super) fn value_items(
        &mut self,
        values: TypeId,
        span: Span,
    ) -> (Vec<(Span, TypeId)>, bool) {
        let Types::Pack(l) = self.session.type_arenas.get_type(values) else {
            return (vec![(span, values)], false);
        };
        let list = self.session.type_arenas.list(l).clone();
        let items = list.fixed.iter().map(|&t| (span, t)).collect();
        (items, list.vararg.is_some())
    }

    /// 标注的返回类型。`sig` 是带 rettype 槽的那个节点（funcbody / methodsig /
    /// functype）：rettype 在文法上是单独的候选式，所以「没写 `->`」就是没这个孩子，
    /// 和 `-> ()`（有孩子、类型是空 Pack）分得开
    pub(super) fn declared_ret(&self, ast: &Tree<NodeSyntax<'_>>, sig: usize) -> Option<TypeId> {
        ast.get_node(sig)
            .children
            .iter()
            .find(|&&c| self.get_production(ast, c) == Some(Prod::ReturnType))
            .map(|&c| self.child_type(c))
    }

    /// 没写 `->` 时的返回列表：体里各条 return 攒在帧上，这里合成一张列表。
    ///
    /// 一条 return 都没有就是 void。多条形状不一时逐位取并：
    /// `if c then return 1 end return nil` 推出 `number|nil`。位数不齐的那几位补 nil ——
    /// 少给的那条路真的就是返 nil。只认栈顶那格：开参数位的 functype
    /// （`function(number)`）没体也没帧，对不上就算 void
    pub(super) fn inferred_ret(&mut self, body: usize) -> ListId {
        let seen = match self.ret_stack.last() {
            Some(frame) if frame.node == body => frame.seen.clone(),
            _ => return ListId::EMPTY,
        };
        let lists: Vec<TypeList> = seen
            .into_iter()
            .map(|ty| {
                let l = self.session.type_arenas.as_list(ty);
                self.session.type_arenas.list(l).clone()
            })
            .collect();
        let width = lists.iter().map(|l| l.fixed.len()).max().unwrap_or(0);
        let mut fixed = Vec::with_capacity(width);
        for i in 0..width {
            let at_i: Vec<TypeId> = lists
                .iter()
                .map(|l| l.fixed.get(i).copied().or(l.vararg).unwrap_or(TypeId::NIL))
                .collect();
            fixed.push(self.session.type_arenas.union(at_i));
        }
        let tails: Vec<TypeId> = lists.iter().filter_map(|l| l.vararg).collect();
        let vararg = (!tails.is_empty()).then(|| self.session.type_arenas.union(tails));
        self.session.type_arenas.intern_list(fixed, vararg)
    }

    /// 体里读到了自己的名字。没写 `->` 的话返回类型的推断就依赖自己，
    /// 环解不开（README：参与递归的函数必须显式标注返回类型，同 TS7023）。
    ///
    /// 只认**自**递归：互递归要翻整张调用图，而那两种形态各自已有人拦 ——
    /// 局部互递归得写前向声明（否则“可能尚未赋值”）、而前向声明必带类型；
    /// 全局互递归靠抬升，抬升的名字拿到的是另一条的声明类型、不经过本帧的推断。
    /// 方法不在此列：`self:m()` 不是裸名字读，走的是字段查找
    pub(super) fn check_recursive_ret(&mut self, name: &str, span: Span, log: &mut Vec<Logger>) {
        // 快路：没一格帧带名字（或都标了返回类型）时不必 intern
        if self
            .ret_stack
            .iter()
            .all(|f| f.annotated || f.name.is_none())
        {
            return;
        }
        let id = self.session.names.intern(name);
        let Some(frame) = self
            .ret_stack
            .iter_mut()
            .find(|f| !f.annotated && f.name == Some(id) && !f.reported)
        else {
            return;
        };
        frame.reported = true;
        log.push(Logger {
            span,
            msg: format!("递归函数 {name} 必须标注返回类型：不标的话返回类型要靠体里的 return 推，而它又依赖自己"),
        });
    }

    /// 进一个函数体：压一格 `RetFrame`。`sig` 是带 rettype 槽的节点（funcbody 是
    /// 它自己，@MethodDef 是 methodsig）；名字只为「递归须标注」那一条而记
    pub(super) fn push_ret_frame(&mut self, ast: &Tree<NodeSyntax<'_>>, body: usize, sig: usize) {
        let annotated = self.declared_ret(ast, sig).is_some();
        let name = self
            .own_name_of(ast, body)
            .map(|nm| self.session.names.intern(nm));
        self.ret_stack.push(RetFrame {
            node: body,
            sig,
            annotated,
            name,
            seen: Vec::new(),
            reported: false,
        });
    }

    /// 出函数体。认下标才弹：不是本体那格就是开合没配对，宁可不弹也不能弹错人
    pub(super) fn pop_ret_frame(&mut self, body: usize) {
        if self.ret_stack.last().is_some_and(|f| f.node == body) {
            self.ret_stack.pop();
        }
    }

    /// 这个函数体自己的名字。两种 @FuncDecl：`LOCAL FUNCTION NAME funcbody`
    /// （名字在 children[2]）与 `FUNCTION funcname funcbody`（funcname 是 @FuncName
    /// 才算裸名）。函数表达式没名字，方法也不算 —— 它们不经裸名字自引用
    pub(super) fn own_name_of<'a>(
        &self,
        ast: &'a Tree<NodeSyntax<'a>>,
        body: usize,
    ) -> Option<&'a str> {
        let parent = ast.get_node(body).parent?;
        if self.get_production(ast, parent) != Some(Prod::FuncDecl) {
            return None;
        }
        let children = &ast.get_node(parent).children;
        if children.len() == 4 {
            return self.get_child_name(ast, parent, 2);
        }
        let fname = children[1];
        (self.get_production(ast, fname) == Some(Prod::FuncName))
            .then(|| self.get_child_name(ast, fname, 0))?
    }
}
