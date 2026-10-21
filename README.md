# TypeLua

给 Lua 加上静态类型检查，编译产物仍然是普通的 Lua 5.3。

写的还是熟悉的 Lua：`local`、`function`、表构造、`require` 都照旧。`typedef`、类型标注、
泛型实参、`import` 在发射时擦掉，`class` 降级成普通的表和元表。

- **错误提前发现**：类型不匹配、字段缺失、未赋值就读取、跨模块名字写错，编译期就报出来。
- **运行时零负担**：类型信息不进生成的代码；单文件编译不引入任何运行时依赖，
  多文件工程也只多一个共享运行时文件。
- **产物可读**：生成的就是 Lua 5.3，直接交给现有的 `lua` 工具链。
- **和现有代码共存**：只新增 6 个关键字，`extends` 这类词仍然可以当普通标识符用。

## 快速开始

### 安装

需要 Rust 1.85+（edition 2024）：

```bash
cargo build --release
# 产物：target/release/typelua
```

macOS 用户也可以直接用 GitHub Releases 里的通用二进制（arm64 + x86_64），下载后 `chmod +x` 即可。
下面的命令假设 `typelua` 已经在 `PATH` 里（或把 `target/release` 加进去）。

### 第一个程序

新建 `hello.tua`：

```lua
class Greeter {
    name:string = "world",
    greet(self) -> string
        return "hello, " .. self.name
    end,
}

local g = Greeter{}
print(g:greet())
```

检查、编译、运行：

```bash
typelua check hello.tua
typelua compile hello.tua
lua hello.lua        # hello, world
```

`compile` 默认在源文件旁生成同名 `.lua`；加 `--out dist`（或 `-o dist`）可以指定输出目录。

### 类型错误在编译期拦下

```lua
local age:number = "42"
```

```text
[error] age: 不能把 string 赋给标注的 number 位 @ 6..9
[error] 检查失败：1 条错误，0 条警告
```

`check`、`compile`、`parse` 失败时退出码为 `1`，可以直接作为 CI 的闸门。

## 常用命令

| 命令 | 作用 |
| --- | --- |
| `typelua check <路径>...` | 解析 + 类型检查，不生成文件 |
| `typelua compile <路径>... [--out DIR]` | 检查通过后生成 `.lua` |
| `typelua parse <路径>...` | 只做语法解析（编辑器 / 调试用） |
| `typelua plugin <目录>...` | 输出 JSON（高亮、跳转、诊断），供编辑器插件消费 |

- 路径可以是文件或目录，目录会递归收集 `.tua`。
- 多文件工程请把**整个目录**传进去：只传其中一个文件时，`import` 找不到同项目的其他模块。
- `--out` / `-o` 只用于 `compile`；多模块工程会在输出目录多生成一个 `__typelua_runtime.lua`。
- 日志写 stderr：`-q` 只看警告和错误，`-v` 打开调试输出，`--log=LEVEL` 精确指定级别；
  也可以用环境变量 `TYPELUA_LOG=error|warn|info|debug`。
- 退出码：`0` 成功，`1` 失败（语法错误、类型错误、路径读不了、写文件失败）。

## 多文件项目

`mod_a.tua`：

```lua
pub class User {
    name:string,
    greet(self) -> string
        return "hi, " .. self.name
    end,
}

local M = User{name = "Ada"}
return M
```

`mod_b.tua`：

```lua
import {User} in "mod_a"

local u = User{name = "Ada"}
print(u:greet())        -- hi, Ada
```

```bash
typelua compile . --out dist
cd dist && lua mod_b.lua
```

`pub` 决定类型是否跨文件可见；`pub class` 同时导出值，所以 import 之后可以直接
`User{...}` 构造、继承，或者访问它的静态成员。跨模块引用由编译器生成的
`__typelua.get("mod_a", "User")` 解析，不需要手写同名的 `require` 绑定。
实例数据和模块表仍然走 Lua 的 `require` / `return`。

## 编辑器支持

VSCode 扩展提供语法高亮、语义着色、跳转定义、诊断和补全：

```bash
python3 editor/vscode/package-vsix.py
code --install-extension editor/vscode/typelua-1.0.0.vsix
```

扩展会调用 `typelua plugin` 分析工作区；搜索范围可以在 `.vscode/typelua.json` 里配置：

```json
{
  "search_path": ["./src", "./examples"]
}
```

命令面板里还有 **TypeLua: Re-analyze workspace** 和 **TypeLua: Build to dist/ (Lua 5.3)**。
扩展只内置 macOS 二进制；其他平台把 `typelua.binaryPath` 指向自己构建的 CLI 即可。
细节见 [editor/vscode/README.md](editor/vscode/README.md)。

## 示例

| 文件 | 内容 |
| --- | --- |
| `examples/sample.tua` | 语言特性总览（类型、类、泛型、模块） |
| `examples/demo.tua` | 诊断反例集，`check` 会故意报错 |
| `examples/pkg/account.tua` | 多文件 / `pub` 导出示例 |

## 深入了解

- [designs.md](designs.md)：设计哲学、语言参考、与 Lua 的文法差异、词法与完整文法。
- [editor/vscode/README.md](editor/vscode/README.md)：插件配置与实现说明。
