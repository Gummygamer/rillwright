local storage = require("storage")
local tools = require("tools")
local ui = require("ui")
local keys = {
    {"q","w","e","r","t","y","u","i","o","p"},
    {"a","s","d","f","g","h","j","k","l","?"},
    {"z","x","c","v","b","n","m",",",".","!"},
    {"Shift","Space","Delete","Send","New","Sessions"}
}
local row, col, shift = 1, 1, false
local draft, status = "", ""
local history, worker = {}, nil
local scroll, approvalScroll = 0, 0
local reading = false

local id, menu, pending, task
local seen = {}
local function selectSession(nextId)
    local loaded = storage.load(nextId)
    storage.replace("sessions/current",nextId)
    id, history, draft, status, scroll, menu, seen = nextId, loaded, "", "", 0, nil, {}
end

local function guarded(fn)
    local ok, err = pcall(fn)
    if not ok then status = tostring(err) end
end

function love.load()
    guarded(function()
        local last = love.filesystem.read("sessions/current")
        if last and last:match("^%d+%-%d+$") then selectSession(last) end
    end)
end

local function turn(role, value)
    return "<|im_start|>" .. role .. "\n" .. value:gsub("<|", "< |") .. "<|im_end|>\n"
end

local assistant = "<|im_start|>assistant\n<think>\n"
local stream = love.thread.getChannel("stream")

-- Separate the reasoning from the reply. The prompt opens <think>, so streamed
-- text is reasoning until </think>; a finished reply without one is all answer.
local function split(text, final)
    text = text:gsub("^%s*<think>", "")
    local thinking, body = text:match("^(.-)</think>(.*)$")
    if thinking then return thinking, (body:gsub("^%s+", "")), true end
    if final then return "", text, true end
    return text, "", false
end

-- Drop a trailing partial UTF-8 sequence so a half-streamed glyph can be drawn.
local function complete(s)
    for i = #s, math.max(1, #s - 3), -1 do
        local b = s:byte(i)
        if b < 0x80 then return s end
        if b >= 0xC0 then
            local need = b >= 0xF0 and 4 or b >= 0xE0 and 3 or 2
            return #s - i + 1 >= need and s or s:sub(1, i - 1)
        end
    end
    return s
end

-- Readable form of a possibly unfinished <tool_call> body.
local function describe(body)
    local lines = {body:match("<function=([%w_]*)") or "…"}
    local pos = 1
    while true do
        local _, open, key = body:find("<parameter=([%w_]+)>", pos)
        if not open then break end
        local close = body:find("</parameter>", open + 1, true)
        local value = body:sub(open + 1, (close or #body + 1) - 1):gsub("^\n", "")
        lines[#lines + 1] = key .. ": " .. value
        if not close then break end
        pos = close + 12
    end
    return table.concat(lines, "\n")
end

local function live()
    if not task then return nil end
    if task.view and task.view.version == task.version then return task.view end
    local blocks = {}
    local function add(label, text, color)
        if text:match("%S") then blocks[#blocks + 1] = {label=label, text=complete(text), color=color} end
    end
    for _, step in ipairs(task.steps) do
        add("THINKING", step.thinking, "muted")
        add("TOOL CALL", describe(step.call.raw), "amber")
        if step.output then add("TOOL RESULT", step.output:sub(1, 512), "amber") end
    end
    if worker then
        local thinking, body, closed = split(task.stream)
        add(closed and "THINKING" or "THINKING …", thinking, "muted")
        if body:find("<tool_call>", 1, true) then add("TOOL CALL …", describe(body), "amber")
        else add("DORY …", body, "mint") end
        if task.stream == "" then status = "Dory is working…"
        elseif not closed then status = "Dory is thinking…"
        elseif body:find("<tool_call>", 1, true) then status = "Dory is calling a tool…"
        else status = "Dory is answering…" end
    end
    task.view = {version=task.version, user=draft, blocks=blocks}
    return task.view
end

local function generate()
    local prefix = turn("system",tools.instructions)
    local prompt = turn("user",draft) .. task.context .. assistant
    for i = #history, 1, -1 do
        local pair = turn("user",history[i].user) .. turn("assistant",history[i].answer)
        if #prefix + #prompt + #pair > 3400 then break end
        prompt = pair .. prompt
    end
    stream:clear()
    task.stream, task.version = "", task.version + 1
    worker = love.thread.newThread("worker.lua")
    worker:start(prefix .. prompt)
    status = "Dory is working…"
end

local function send()
    if task or menu or not draft:match("%S") then return end
    guarded(function()
        if not id then id = storage.new(); storage.replace("sessions/current",id) end
        task = {trace={}, context="", rounds=0, steps={}, stream="", version=0}
        generate()
    end)
end

local function finish(answer)
    history[#history+1] = {user=draft,answer=answer,trace=table.concat(task.trace,"\n")}
    task, pending, worker, draft, status, scroll = nil,nil,nil,"","",0
    guarded(function() storage.save(id,history) end)
end

local function result(call, approved)
    local ok,value
    if approved then ok,value=pcall(tools.run,call.name,call.args,seen)
    else value="User declined" end
    local output=(ok and "" or "Error: ") .. tostring(value)
    task.trace[#task.trace+1]=call.name .. "  " .. output:sub(1,512)
    call.step.output=output
    -- Keep the latest observation within the native model's context.
    task.context=turn("assistant",call.raw) .. turn("tool",output:sub(1,math.max(128,2750-#draft-#call.raw)))
    if #task.context + #draft > 2850 then
        task.context=turn("tool",call.name .. ": " .. output:sub(1,math.max(128,2600-#draft)))
    end
    pending=nil
    generate()
end

local function receive(reply)
    local thinking,text=split(reply,true)
    local ok,name,args=pcall(tools.parse,text)
    if not ok then finish("Tool request failed: " .. tostring(name)); return end
    if not name then finish(text); return end
    task.rounds=task.rounds+1
    if task.rounds>6 then finish("Tool round limit reached."); return end
    local call={name=name,args=args,raw=text}
    call.step={thinking=thinking,call=call}
    task.steps[#task.steps+1]=call.step
    task.version=task.version+1
    if name=="write_file" or name=="edit_file" then
        pending=call
        approvalScroll=0
    else result(call,true) end
end

local function sessions()
    local ids=storage.sessions()
    if #ids==0 then status="No saved sessions yet"; return end
    menu={ids=ids,index=1}
end

local function modal(k)
    if pending then
        if k=="a" then result(pending,true)
        elseif k=="b" then result(pending,false)
        elseif k=="leftshoulder" then approvalScroll=math.max(0,approvalScroll-4)
        elseif k=="rightshoulder" then approvalScroll=approvalScroll+4 end
    elseif menu then
        if k=="b" then menu=nil
        elseif k=="a" then guarded(function() selectSession(menu.ids[menu.index]) end)
        elseif k=="dpup" then menu.index=math.max(1,menu.index-1)
        elseif k=="dpdown" then menu.index=math.min(#menu.ids,menu.index+1) end
    else return false end
    return true
end

local function insert(s)
    if not task and not menu and #draft + #s <= 2000 then draft = draft .. s end
end

local function activate(k)
    if task or menu then return end
    if k == "Send" then send()
    elseif k == "New" then guarded(function() selectSession(storage.new()) end)
    elseif k == "Sessions" then guarded(sessions)
    elseif k == "Shift" then shift = not shift
    elseif k == "Space" then insert(" ")
    elseif k == "Delete" then draft = draft:gsub("[%z\\1-\\127\\194-\\244][\\128-\\191]*$", "")
    else insert(shift and k:upper() or k) end
end

function love.gamepadpressed(_, k)
    if modal(k) then return end
    if k == "x" then reading = not reading; return end
    if reading then
        if k == "leftshoulder" or k == "dpup" then scroll = scroll + 5
        elseif k == "rightshoulder" or k == "dpdown" then scroll = math.max(0, scroll - 5)
        elseif k == "a" or k == "b" then reading = false end
        return
    end
    if k == "dpup" then row = math.max(1, row - 1)
    elseif k == "dpdown" then row = math.min(#keys, row + 1)
    elseif k == "dpleft" then col = math.max(1, col - 1)
    elseif k == "dpright" then col = math.min(#keys[row], col + 1)
    elseif k == "a" then activate(keys[row][col])
    elseif k == "b" then activate("Delete")
    elseif k == "start" then send()
    elseif k == "leftshoulder" then scroll = scroll + 5
    elseif k == "rightshoulder" then scroll = math.max(0, scroll - 5) end
    col = math.min(col, #keys[row])
end

function love.textinput(s)
    if not menu and not task then reading = false; insert(s) end
end
function love.keypressed(k)
    if modal(({["return"]="a",escape="b",up="dpup",down="dpdown",pageup="leftshoulder",pagedown="rightshoulder"})[k]) then return end
    if k == "tab" then reading = not reading
    elseif k == "pageup" then scroll = scroll + 5
    elseif k == "pagedown" then scroll = math.max(0, scroll - 5)
    elseif k == "return" then
        if reading then reading = false else send() end
    elseif k == "backspace" and not reading then activate("Delete") end
end

function love.update()
    if not worker then return end
    while true do
        local piece = stream:pop()
        if piece == nil then break end
        task.stream = piece and task.stream .. piece or ""
        task.version = task.version + 1
    end
    local reply = love.thread.getChannel("reply"):pop()
    if reply then
        worker=nil
        if reply.ok then receive(reply.text)
        elseif #task.trace>0 then finish(reply.text)
        else task=nil; status=reply.text end
    elseif worker:getError() then finish(worker:getError()) end
end

local function pointerPressed(x, y)
    local action, r, c = ui.hit(x, y, pending or menu or reading)
    if not action then return end
    if menu or pending then modal(action)
    elseif action == "view" then reading = not reading
    elseif action == "older" then scroll = scroll + 5
    elseif action == "newer" then scroll = math.max(0, scroll - 5)
    elseif action == "key" then row, col = r, c; activate(keys[r][c]) end
end

function love.mousepressed(x, y, button, istouch)
    if button == 1 and not istouch then pointerPressed(x, y) end
end

function love.touchpressed(_, x, y) pointerPressed(x, y) end

function love.wheelmoved(_, y)
    if y == 0 then return end
    if pending then modal(y > 0 and "leftshoulder" or "rightshoulder")
    elseif menu then modal(y > 0 and "dpup" or "dpdown")
    else scroll = math.max(0, scroll + (y > 0 and 3 or -3)) end
end

function love.draw()
    scroll, approvalScroll = ui.draw({
        id=id, history=history, draft=draft, status=status, active=task ~= nil,
        keys=keys, row=row, col=col, shift=shift, reading=reading,
        scroll=scroll, approvalScroll=approvalScroll, menu=menu, pending=pending,
        live=live()
    })
end
