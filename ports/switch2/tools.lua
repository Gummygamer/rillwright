local M = {}
local fs = love.filesystem
local storage = require("storage")
local schema = {
    list_dir = {}, read_file = {"path"}, search_files = {"query"},
    write_file = {"path","content"}, edit_file = {"path","old","new"}
}
M.instructions = [[You are Rillwright. Answer normally or use one workspace tool at a time.
Tools: list_dir(), read_file(path), search_files(query), write_file(path,content), edit_file(path,old,new).
Use <tool_call><function=read_file><parameter=path>notes.txt</parameter></function></tool_call>
with one parameter element for each argument. Use relative paths. Read before overwriting.]]
function M.parse(text)
    if not text:find("<tool_call>",1,true) then return end
    local name, body = text:match("^%s*<tool_call>%s*<function=([%w_]+)>(.-)</function>%s*</tool_call>%s*$")
    assert(schema[name], "Malformed or unknown tool call")
    local args = {}
    while body:match("%S") do
        local key, value, nextpos = body:match("^%s*<parameter=([%w_]+)>(.-)</parameter>()")
        assert(key and not args[key], "Invalid tool parameter")
        args[key] = value:gsub("^\n",""):gsub("\n$","")
        body = body:sub(nextpos)
    end
    local expected = {}
    for _, k in ipairs(schema[name]) do assert(args[k], "Missing "..k); expected[k]=true end
    for k in pairs(args) do assert(expected[k], "Unexpected "..k) end
    return name,args
end

local function path(name)
    assert(#name <= 180 and name:match("^[%w_%-]") and not name:find("[^%w._ /%-]")
        and not name:find("//",1,true) and name:sub(-1) ~= "/", "Invalid workspace path")
    for part in name:gmatch("[^/]+") do assert(part:sub(1,1) ~= "." and not part:match("[. ]$"), "Invalid path component") end
    assert(not name:lower():match("%.tmp$"), "Reserved temporary filename")
    return "workspace/"..name
end

local function read(p)
    local info = fs.getInfo(p)
    assert(info and info.type=="file" and info.size<=65536, "Expected a text file up to 64 KiB")
    local value = assert(fs.read(p))
    assert(not value:find("%z"), "Binary file")
    return value
end

local function enumerate(dir, files, depth)
    assert(depth<32, "Directory depth limit")
    local entries = fs.getDirectoryItems(dir)
    table.sort(entries)
    for _, entry in ipairs(entries) do
        local p = dir.."/"..entry
        if entry:sub(1,1) ~= "." and not entry:lower():match("%.tmp$") then
            local info = fs.getInfo(p)
            if info and info.type=="directory" then enumerate(p,files,depth+1)
            elseif info and info.type=="file" then
                assert(#files<1000, "Workspace exceeds 1000 files")
                files[#files+1]=p
            end
        end
    end
end

function M.run(name,a,seen)
    if name=="list_dir" or name=="search_files" then
        assert(fs.createDirectory("workspace"))
        local files, matches = {}, {}
        enumerate("workspace",files,0)
        for _,p in ipairs(files) do
            local include = name=="list_dir"
            if not include then
                assert(a.query~="", "Empty search query")
                local ok,s=pcall(read,p)
                include=ok and s:find(a.query,1,true)
            end
            if include then matches[#matches+1]=p:sub(11) end
        end
        return #matches>0 and table.concat(matches,"\n") or "(no files)"
    end
    local p=path(a.path)
    if name=="read_file" then
        local s=read(p)
        seen[p]=s
        return s
    end
    local exists=fs.getInfo(p)
    local old=exists and read(p)
    assert(old==seen[p], "Read the current file before changing it")
    local content=a.content
    if name=="edit_file" then
        assert(old and a.old~="", "Expected existing file and nonempty match")
        local start,stop=old:find(a.old,1,true)
        assert(start and not old:find(a.old,stop+1,true), "Expected one exact match")
        content=old:sub(1,start-1)..a.new..old:sub(stop+1)
    end
    assert(#content<=65536,"File exceeds 64 KiB")
    storage.replace(p,content)
    seen[p]=content
    return "Saved "..a.path
end
return M
