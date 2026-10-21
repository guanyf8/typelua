-- TypeLua 共享运行时。多模块工程的每个 .lua 文件头都会 require 它：
--
--   local __typelua = require("__typelua_runtime")
--   __typelua.enter("pkg.account")
--
-- 它只做两件事：
--   1. def / get：pub 类在自己的定义处登记，别的模块按 (模块名, 类名) 取
--   2. get 时类还没登记：那个模块没开始加载就 require 它；已经开始加载就报错 ——
--      后一种就是循环 require，报一条清楚的比 Lua 自己递归到爆栈有用
local M = {}

-- 模块名 -> { 类名 = 类表 }
local defs = {}
-- 模块名 -> true：该模块的代码已经开始执行（文件头的 enter 置位）
local started = {}

function M.enter(module)
    started[module] = true
end

function M.def(module, name, class)
    local t = defs[module]
    if not t then
        t = {}
        defs[module] = t
    end
    t[name] = class
end

function M.get(module, name)
    local t = defs[module]
    if t and t[name] ~= nil then
        return t[name]
    end
    -- 已经开始加载、却没登记：对面模块执行到一半回头要这个类，再 require
    -- 也只会拿到半截模块（或者无限递归），只能报错
    if started[module] and not package.loaded[module] then
        error(
            string.format("模块 %s 正在加载，类 %s 尚未注册（循环 require）", module, name),
            0
        )
    end
    local ok, err = pcall(require, module)
    if not ok then
        error(string.format("加载模块 %s 失败：%s", module, tostring(err)), 0)
    end
    t = defs[module]
    if t and t[name] ~= nil then
        return t[name]
    end
    error(string.format("模块 %s 没有注册类 %s（那个类没标 pub？）", module, name), 0)
end

return M
