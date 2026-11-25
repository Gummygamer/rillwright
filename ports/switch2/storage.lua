local fs = love.filesystem
local M = {}

function M.replace(path, data)
    assert(fs.createDirectory(path:match("^(.*)/")))
    local temp = path .. ".tmp"
    assert(fs.write(temp, data))
    local root = fs.getSaveDirectory() .. "/"
    assert(os.rename(root .. temp, root .. path))
end

function M.sessions()
    local ids = {}
    for _, file in ipairs(fs.getDirectoryItems("sessions")) do
        local id = file:match("^(%d+%-%d+)%.chat$")
        if id then ids[#ids+1] = id end
    end
    table.sort(ids, function(a,b) return a>b end)
    return ids
end

function M.new()
    local stamp = os.date("%Y%m%d%H%M%S")
    for i = 1, 9999 do
        local id = stamp .. ("-%04d"):format(i)
        if not fs.getInfo("sessions/" .. id .. ".chat") then
            M.save(id,{})
            return id
        end
    end
    error("Session ID limit reached")
end

function M.save(id, history)
    local fields = {"RW1\n"}
    for _, pair in ipairs(history) do
        for _, s in ipairs({pair.user, pair.answer, pair.trace or ""}) do
            fields[#fields+1] = #s .. ":" .. s
        end
    end
    M.replace("sessions/" .. id .. ".chat", table.concat(fields))
end

function M.load(id)
    local data = assert(fs.read("sessions/" .. id .. ".chat"))
    assert(data:sub(1,4) == "RW1\n", "Invalid session header")
    local history, pos = {}, 5
    local function field()
        local start, stop, n = data:find("(%d+):", pos)
        assert(start == pos and stop + tonumber(n) <= #data, "Incomplete session")
        pos = stop + tonumber(n) + 1
        return data:sub(stop+1,pos-1)
    end
    while pos <= #data do
        local user = field()
        local answer = field()
        history[#history+1] = {user=user, answer=answer, trace=field()}
    end
    return history
end
return M
