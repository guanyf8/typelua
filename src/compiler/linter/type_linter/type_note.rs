//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §8 类型位 ==================

    /// `basictype : NAME` / `extendtype : NAME` —— 类型位的裸名字。
    ///
    /// 这条产生式原本无标签单孩子、会被 parser 折叠掉，于是它和变量位的
    /// `prefixexp : NAME` 在 AST 上形状完全相同；而 `type -> uniontype ->
    /// intertype -> basictype` 三层同样是折叠的，parent 会被打穿成
    /// @TypeAnnotation / @Union / @Intersect / @ListTail / @ArgTypeNamed …
    /// 的开放集合，看 parent 也判不出是不是类型位。只有 @TypeName 这个标签
    /// 能把「查 type_names」和「查 variables」分开
    pub(super) fn resolve_type_name(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let name = self.name_or_panic(ast, ast.get_node(node_index).children[0], "type name");
        let span = ast.span_of(node_index);
        self.instantiate_type_name(ctx, name, ListId::EMPTY, span, diags)
    }

    /// NAME '<' typeargs '>'
    pub(super) fn resolve_generic_type(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let children = ast.get_node(node_index).children.clone();
        let name = self.name_or_panic(ast, children[0], "generic type");
        // 单实参折叠成裸类型、多实参是 Pack，as_list 都能收成 ListId
        let arg_ty = self.child_type(ctx, children[2]);
        let args = self.session.type_arenas.as_list(arg_ty);
        let span = ast.span_of(node_index);
        self.instantiate_type_name(ctx, name, args, span, diags)
    }

    /// 名义类型名 -> TypeId。`@TypeName`（不带实参，args 是 EMPTY）和
    /// `@GenericType`（带实参）共用这一条：查名字、校 arity、三个 TypeRef 变体
    /// 各自的去处，两边逐字一样，差别只在 args。
    ///
    /// 查不到 / arity 不对一律出诊断而不是 panic：这里全是用户能写错的东西，
    /// `extern x:NoSuchType` 是 README 明确要求报错的笔误，panic 会把一个笔误
    /// 变成编译器崩溃。名字取不到才 panic —— 那是文法保证的不变式
    pub(super) fn instantiate_type_name(
        &mut self,
        ctx: &Context,
        name: &str,
        args: ListId,
        span: Span,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let na = self.session.type_arenas.list(args).len();
        let Some(found) = self.lookup_type(ctx, name) else {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
                span,
                msg: format!("未声明的类型 {}", name),
            });
            return TypeId::UNKNOWN;
        };
        match found {
            // 标量和泛型形参都不接受实参：`number<string>` / `T<number>`
            TypeRef::Prim(_) | TypeRef::Generic(_) if na != 0 => {
                diags.push(Diagnostic {
                    severity: DiagLevel::Error,
                    span,
                    msg: format!("{} 不是 class 或 typedef，不能带类型实参", name),
                });
                TypeId::UNKNOWN
            }
            TypeRef::Prim(p) => TypeId::of_trival(p),
            // class 体内的 `T`：形参本身就是个类型，等实例化时才被 subst 换掉
            TypeRef::Generic(g) => self.session.type_arenas.generic(g),
            // class 和 typedef 共用 TypeRef::Decl，是哪种由 DeclTable 说了算；
            // 类型值只记 Ref{decl, args}，真正的实例化（subst）推迟到查字段时才做
            TypeRef::Decl(decl) => {
                let ng = self.session.decls.get(decl).generics.len();
                if ng != na {
                    diags.push(Diagnostic {
                        severity: DiagLevel::Error,
                        span,
                        msg: format!("{} 需要 {} 个类型实参，实际给了 {}", name, ng, na),
                    });
                    return TypeId::UNKNOWN;
                }
                // 个数对上了才谈上界：形参与实参逐位配对，越了界的在这儿报。
                // 上界本身不进类型值：它只是实例化处的一道门槛，Ref 里存的仍是实参
                let gids = self.session.decls.get(decl).generics.clone();
                let argtys = self.session.type_arenas.list(args).fixed.clone();
                self.check_generic_bounds(&gids, &argtys, span, diags);
                self.session.type_arenas.reference(decl, args)
            }
        }
    }

    /// 逐位核对类型实参有没有越过形参的上界。没写上界（constraint 是 None）就放过
    /// —— 那是「任意类型」。个数由调用点先对齐，这里 zip 就行；实参本身是个
    /// 未解的形参（`class NamedBox<T> : Box<T>` 那种透传）时，assignable 对 Generic
    /// 本就放过，于是透传不会在这儿误报
    pub(super) fn check_generic_bounds(
        &mut self,
        gids: &[GenericId],
        args: &[TypeId],
        span: Span,
        diags: &mut Vec<Diagnostic>,
    ) {
        for (&g, &arg) in gids.iter().zip(args) {
            let Some(bound) = self.session.decls.generic(g).constraint else {
                continue;
            };
            if !self.session.assignable(arg, bound) {
                let param = self.session.decls.generic(g).name();
                diags.push(Diagnostic {
                    severity: DiagLevel::Error,
                    span,
                    msg: format!(
                        "类型实参 {} 越过了形参 {} 的上界 {}",
                        self.session.show(arg),
                        self.session.names.resolve(param),
                        self.session.show(bound)
                    ),
                });
            }
        }
    }

    /// `typeparam : NAME ':' type` —— 形参上界。children[0] 是形参名、children[2]
    /// 是上界类型，解出来填进 `GenericInfo.constraint`。约束位是单个 type
    /// （union 算一个），所以 constraint 是 `Option<TypeId>` 而不是 `Option<Vec<..>>`。
    ///
    /// 身份从作用域反查：这一格被遍历到时，形参名已经由 scope_decl_generics /
    /// scope_func_generics 挂进当前作用域了（generics 就在 class / funcbody 自己那格里）。
    ///
    /// 顶层 class / typedef 的 generics 在 hoist_class_bodies 的 eager 趟就被求过一遍，
    /// 于是主遍历开始前所有顶层上界已落表 —— `Bounded<string>` 写在声明**之前**
    /// 也校得动。节点本身没类型可给（形参位是个名字，不是个类型表达式）
    pub(super) fn resolve_type_param_bound(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        _diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        if let Some(name) = self.get_child_name(ast, node_index, 0) {
            // 上界自己就是个类型，它得先被求出来（写错的名字由 resolve_type_name 报）
            let bound = self.pass_through(ctx, ast, node_index, 2);
            if let Some(TypeRef::Generic(g)) = self.lookup_type(ctx, name) {
                self.session.decls.generic_mut(g).constraint = Some(bound);
            }
        }
        TypeId::UNKNOWN
    }

    /// `prefixexp TURBOFISH typeargs '>'` —— 显式给出类型实参。children[0] 是被指定
    /// 的那个函数值、children[2] 是实参表（单个折成裸类型，`as_list` 收口）。
    ///
    /// 只对**泛型函数**成立：class / typedef 的实参写 `Box<number>`，走
    /// resolve_generic_type，不从这儿来。形参的顺序记在 `Types::Func` 的 generics
    /// 那格 —— 使用点只拿得到一个函数值，靠它才知道第 i 个实参代给谁。
    ///
    /// 代完之后 subst 会把 generics 那格摘空，于是 `f::<number>::<string>` 第二次
    /// 自然报「不是泛型函数」。结果就是代入后的 Func，外层 @Call 照常 ret_of
    pub(super) fn resolve_turbo_fish(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let children = &ast.get_node(node_index).children;
        let (callee, args_ty) = (
            self.child_type(ctx, children[0]),
            self.child_type(ctx, children[2]),
        );
        let span = ast.span_of(node_index);
        // 前面已经错过了（未声明的名字之类）：不再堆一层假错
        if callee == TypeId::UNKNOWN {
            return TypeId::UNKNOWN;
        }
        let Types::Func { generics, .. } = self.session.type_arenas.get_type(callee) else {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
                span,
                msg: format!(
                    "{} 不是函数，不能用 ::<> 给类型实参",
                    self.session.show(callee)
                ),
            });
            return TypeId::UNKNOWN;
        };
        // generics 那格存的是 `Types::Generic` 包过的 TypeId，代入要的是 GenericId 本身
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
        let args = self.session.type_arenas.as_list(args_ty);
        let argtys = self.session.type_arenas.list(args).fixed.clone();
        if gids.is_empty() {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
                span,
                msg: "这个函数没有泛型形参，不能给类型实参".to_string(),
            });
            return TypeId::UNKNOWN;
        }
        if gids.len() != argtys.len() {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
                span,
                msg: format!(
                    "这个函数需要 {} 个类型实参，实际给了 {}",
                    gids.len(),
                    argtys.len()
                ),
            });
            return TypeId::UNKNOWN;
        }
        self.check_generic_bounds(&gids, &argtys, span, diags);
        let s = self.session.type_arenas.intern_subst(&gids, &argtys);
        self.session.type_arenas.subst(callee, s)
    }

    /// 三条「函数样」产生式共用的签名组装 —— 它们的形状同构：
    ///   - `functype  : FUNCTION '(' [argtypes] ')' [rettype]`
    ///   - `funcbody  : generics '(' [parlist]  ')' [rettype] block END`
    ///   - `methodsig : NAME     '(' [parlist]  ')' [rettype]`
    ///
    /// `'('` 前那一格各被一个节点占满（FUNCTION / generics / NAME，空 generics 也占
    /// 一个零孩子的 `Prod(None)`），所以 children[2] 恒是 `')'`（无参）或参数表 ——
    /// 单个折成裸类型、多个是 @ListTail 的 Pack、vararg 走 @ParamsVararg，`as_list`
    /// 统一收成 ListId；返回列表认 @ReturnType 标签。括号 / block / END 不参与类型，
    /// 形参已在 `resolve_param` 里落进作用域，这里只管拼类型。
    ///
    /// 类型系统里没有「方法」，`a:f(x)` 就是 `a.f(a, x)`，所以 methodsig 产出的
    /// 也是个普通 Func。用户自己写的 `function f() -> never … end` 能终结控制流
    /// （而不只是 `extern` 声明的），靠的就是这里把 ret 算出来。
    ///
    /// 不能靠 `contains_key` 跳过 FUNCTION / 括号：每个节点（含标点）都会被
    /// register_value 登记成 UNKNOWN，跳不掉，末尾那个 `')'` 反把 params 冲成 [unknown]
    pub(super) fn func_sig_type(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        // children 取所有权：循环里要用 &mut self.session，不能一直挂着 ast 的借用
        let children = ast.get_node(node_index).children.clone();
        let mut params = ListId::EMPTY;
        let mut ret = ListId::EMPTY;
        let after_paren = children[2];
        let is_close = matches!(
            ast.get_node(after_paren).get_data(),
            NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE(')')))
        );
        if !is_close {
            let ty = self.child_type(ctx, after_paren);
            params = self.session.type_arenas.as_list(ty);
        }
        match self.declared_ret(ctx, ast, node_index) {
            Some(ty) => ret = self.session.type_arenas.as_list(ty),
            // 没写 `->`：拿体里的 return 推。funcbody 才有体—— functype /
            // methodsig 对不上帧，`inferred_ret` 会给空列表（即 void），
            // 方法那一路由 `resolve_method_def` 在体跑完之后装回去
            None => ret = self.inferred_ret(ctx, node_index),
        }
        // 只有 funcbody 那一条的 children[0] 是 generics（functype 那格是 FUNCTION、
        // methodsig 是 NAME），generic_list_of 认 @Generics 标签，不是就算空，
        // 所以三条产生式仍能共用这一句
        let generics = self.generic_list_of(ctx, ast, children[0]);
        self.session.type_arenas.func(generics, params, ret)
    }

    /// `generics` 那格里的形参按声明序收成一条 ListId（每项是 `Types::Generic`），
    /// 填进 `Types::Func` 的 generics 那格，turbofish 靠它认实参的位置。
    ///
    /// 身份从**当前作用域反查**，不能再 fresh：scope_func_generics /
    /// scope_decl_generics 已经把形参按名字挂进来了，再铸一份的话签名里的 T
    /// 和体里的 T 就是两个不同的形参。查不到的直接漏掉（eager 那趟没进
    /// funcbody、没挂过函数级泛型）：那趟只借类型中转，主遍历会重算
    pub(super) fn generic_list_of(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        generics_node: usize,
    ) -> ListId {
        let names = self.generic_param_names(ast, generics_node);
        if names.is_empty() {
            return ListId::EMPTY;
        }
        let mut tys = Vec::with_capacity(names.len());
        for (nm, _) in names {
            if let Some(TypeRef::Generic(g)) = self.lookup_type(ctx, nm) {
                tys.push(self.session.type_arenas.generic(g));
            }
        }
        self.session.type_arenas.intern_list(tys, None)
    }

    /// `functiondef : FUNCTION funcbody` —— 函数表达式的值就是函数体的类型
    pub(super) fn resolve_func_expr(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        self.pass_through(ctx, ast, node_index, 1)
    }

    /// uniontype : uniontype '|' basictype —— union() 自带扁平化/排序/去重，
    /// 左边是不是 Union 都无所谓，收齐两侧类型丢进去即可
    pub(super) fn resolve_union(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        let left = self.pass_through(ctx, ast, node_index, 0);
        let right = self.pass_through(ctx, ast, node_index, 2);
        self.session.type_arenas.union(vec![left, right])
    }

    /// intertype : intertype '&' basictype —— 和 union 同型，intersect() 自带扁平化/
    /// 急合并/去重，两侧收齐丢进去即可。矛盾交（`number & string`）不在这报，
    /// 等 assignable / 声明处用 intersection_conflict 去查
    pub(super) fn resolve_intersect(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        let left = self.pass_through(ctx, ast, node_index, 0);
        let right = self.pass_through(ctx, ast, node_index, 2);
        self.session.type_arenas.intersect(vec![left, right])
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
    pub(super) fn resolve_list(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        _diags: &mut Vec<Diagnostic>,
    ) -> TypeId {
        // `stat_list stat` 是十一条 @ListTail 里唯一没有分隔符的（len == 2）：串的是
        // 语句、不产出类型，形状本身就分得开，不用为它单开一个标签
        if ast.get_node(node_index).children.len() == 2 {
            return TypeId::UNKNOWN;
        }
        // 只有左递归顶才建list
        if !self.is_spine_top(ast, node_index) {
            return TypeId::UNKNOWN;
        }
        let items = self.spine(ast, node_index);
        // spine() 至少产出一个元素，即tail就是本list的节点
        let Some((&tail, head)) = items.split_last() else {
            return TypeId::UNKNOWN;
        };
        let mut fixed = Vec::with_capacity(items.len());
        for &item in head {
            let ty = self.child_type(ctx, item);
            fixed.push(self.truncate_ret(ty));
        }
        let mut vararg = None;
        let last = self.child_type(ctx, tail);
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

    /// `typelist ',' type` —— 末位类型追加到定长部分。multilist 不左递归，
    /// 所以它自己一层就够，不必走 resolve_list 那套脊顶收集
    pub(super) fn pack_fixed(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        let mut fixed = self.fixed_of(ctx, ast, node_index);
        fixed.push(self.pass_through(ctx, ast, node_index, 2));
        self.session.type_arenas.pack(fixed, None)
    }

    /// `list ',' vararg` —— 末位是变长，进 vararg 槽。
    /// vararg 非 None，pack_of 的「长度 1 就折叠」不会触发，Pack 保得住
    pub(super) fn pack_vararg(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        let fixed = self.fixed_of(ctx, ast, node_index);
        let elem = self.vararg_elem(ctx, ast, node_index, 2);
        self.session.type_arenas.pack(fixed, Some(elem))
    }

    /// `'...' type` —— 一格 vararg 自己就是一张「零个定长 + 变长」的列表。
    /// 折成裸类型不行：`-> (...number)` 那样单独成表时会被看成一个定长返回值
    pub(super) fn vararg_pack(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> TypeId {
        let elem = self.pass_through(ctx, ast, node_index, 1);
        self.session.type_arenas.pack(vec![], Some(elem))
    }

    /// 第 `i` 个孩子那格 vararg 的元素类型。带标注的是 @VarargTyped、裸 `...` 是
    /// 折叠后的 token，两者产出的都是只有 vararg 槽的 Pack，这里把元素取回来
    pub(super) fn vararg_elem(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        i: usize,
    ) -> TypeId {
        let ty = self.pass_through(ctx, ast, node_index, i);
        match self.session.type_arenas.get_type(ty) {
            Types::Pack(l) => self
                .session
                .type_arenas
                .list(l)
                .vararg
                .unwrap_or(TypeId::ANY),
            _ => ty,
        }
    }

    /// 取 children[0] 那个列表的定长部分。单个类型会被折成裸类型，as_list 统一收口。
    /// clone 是必须的：list() 借着 session，而 pack() 要 &mut
    pub(super) fn fixed_of(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
    ) -> Vec<TypeId> {
        let head = self.pass_through(ctx, ast, node_index, 0);
        let l = self.session.type_arenas.as_list(head);
        self.session.type_arenas.list(l).fixed.clone()
    }
}
