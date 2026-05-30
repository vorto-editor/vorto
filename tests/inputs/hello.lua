-- Sample Lua file to exercise syntax highlighting, indents,
-- and folds in vorto. Open with `vorto assets/samples/hello.lua`.

local M = {}

local GREETING = "Hello"

--- Build a Person table.
local function new_person(name, age, tags)
    return {
        name = name,
        age = age,
        tags = tags or {},
        is_adult = age >= 18,
    }
end

local function greet(person, prefix)
    prefix = prefix or GREETING
    return string.format("%s, %s!", prefix, person.name)
end

local function classify(n)
    if n < 0 then
        return "negative"
    elseif n == 0 then
        return "zero"
    elseif n % 2 == 0 then
        return "positive even"
    else
        return "positive odd"
    end
end

local function squares(numbers)
    local out = {}
    for _, x in ipairs(numbers) do
        if x * x > 5 then
            out[#out + 1] = x * x
        end
    end
    return out
end

function M.main()
    local alice = new_person("Alice", 30, { "admin", "early_bird" })
    print(greet(alice))

    for n = -2, 5 do
        print(n .. " -> " .. classify(n))
    end

    print("squares: " .. table.concat(squares({ 1, 2, 3, 4, 5 }), ", "))
end

M.main()
return M
