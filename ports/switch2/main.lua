local storage = require("storage")
local tools = require("tools")
local keys = {
    {"q","w","e","r","t","y","u","i","o","p"},
    {"a","s","d","f","g","h","j","k","l","?"},
    {"z","x","c","v","b","n","m",",",".","!"},
    {"Shift","Space","Delete","Send","New","Sessions"}
}
local row, col, shift = 1, 1, false
local draft, status = "", ""
local history, worker = {}, nil
local scroll = 0

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
    love.graphics.setFont(love.graphics.newFont(22))
    guarded(function()
        local last = love.filesystem.read("sessions/current")
        if last and last:match("^%d+%-%d+$") then selectSession(last) end
    end)
end

local function turn(role, value)
    return "<|im_start|>" .. role .. "\n" .. value:gsub("<|", "< |") .. "<|im_end|>\n"
end

local assistant = "<|im_start|>assistant\n<think></think>"
local function generate()
    local prefix = turn("system",tools.instructions)
    local prompt = turn("user",draft) .. task.context .. assistant
    for i = #history, 1, -1 do
        local pair = turn("user",history[i].user) .. turn("assistant",history[i].answer)
        if #prefix + #prompt + #pair > 3400 then break end
        prompt = pair .. prompt
    end
    worker = love.thread.newThread("worker.lua")
    worker:start(prefix .. prompt)
    status = "Dory is working…"
end

local function send()
    if task or menu or not draft:match("%S") then return end
    guarded(function()
        if not id then id = storage.new(); storage.replace("sessions/current",id) end
        task = {trace={}, context="", rounds=0}
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
    -- Keep the latest observation within the native model's context.
    task.context=turn("assistant",call.raw) .. turn("tool",output:sub(1,math.max(128,2750-#draft-#call.raw)))
    if #task.context + #draft > 2850 then
        task.context=turn("tool",call.name .. ": " .. output:sub(1,math.max(128,2600-#draft)))
    end
    pending=nil
    generate()
end

local function receive(text)
    local ok,name,args=pcall(tools.parse,text)
    if not ok then finish("Tool request failed: " .. tostring(name)); return end
    if not name then finish(text); return end
    task.rounds=task.rounds+1
    if task.rounds>6 then finish("Tool round limit reached."); return end
    local call={name=name,args=args,raw=text}
    if name=="write_file" or name=="edit_file" then
        pending=call
        scroll=0
        status="Approve this change?   A approve   B decline   L/R scroll"
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
        elseif k=="leftshoulder" then scroll=math.max(0,scroll-4)
        elseif k=="rightshoulder" then scroll=scroll+4 end
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

function love.textinput(s) insert(s) end
function love.keypressed(k)
    if modal(({["return"]="a",escape="b",up="dpup",down="dpdown",pageup="leftshoulder",pagedown="rightshoulder"})[k]) then return end
    if k == "return" then send() elseif k == "backspace" then activate("Delete") end
end

function love.update()
    if not worker then return end
    local reply = love.thread.getChannel("reply"):pop()
    if reply then
        worker=nil
        if reply.ok then receive(reply.text)
        elseif #task.trace>0 then finish(reply.text)
        else task=nil; status=reply.text end
    elseif worker:getError() then finish(worker:getError()) end
end

local function keyTop() return love.graphics.getHeight() - 184 end
function love.mousepressed(x, y, button)
    if menu or pending then
        if button==1 and y>=keyTop() then
            local controls=menu and {"dpup","dpdown","a","b"} or {"leftshoulder","rightshoulder","a","b"}
            modal(controls[math.floor(x*4/love.graphics.getWidth())+1])
        end
        return
    end
    local r = math.floor((y - keyTop()) / 44) + 1
    if button ~= 1 or not keys[r] then return end
    local c = math.floor(x * #keys[r] / love.graphics.getWidth()) + 1
    if keys[r][c] then row, col = r, c; activate(keys[r][c]) end
end

function love.draw()
    local g, w = love.graphics, love.graphics.getWidth()
    g.clear(0.06, 0.09, 0.13)
    g.setColor(1,1,1)
    g.print("Rillwright   /   Dory", 18, 12)
    if menu or pending then
        if menu then
            g.print("Sessions   /   "..menu.index.." of "..#menu.ids,18,54)
            local first=math.max(1,menu.index-6)
            for i=first, math.min(#menu.ids,first+6) do
                g.print((i==menu.index and ">   " or "     ")..menu.ids[i],18,90+(i-first)*32)
            end
        else
            local _, lines=g.getFont():getWrap(pending.raw,w-40)
            local count=math.max(1,math.floor((keyTop()-80)/28))
            scroll=math.min(scroll,math.max(0,#lines-count))
            for i=scroll+1,math.min(#lines,scroll+count) do g.print(lines[i],18,56+(i-scroll-1)*28) end
        end
        local labels=menu and {"Up","Down","Open (A)","Back (B)"} or {"Up (L)","Down (R)","Approve (A)","Decline (B)"}
        for i,label in ipairs(labels) do g.printf(label,(i-1)*w/4,keyTop()+20,w/4,"center") end
        return
    end
    local lines = {}
    for _, pair in ipairs(history) do
        local messages={"You   "..pair.user}
        if pair.trace and pair.trace~="" then messages[#messages+1]=pair.trace end
        messages[#messages+1]="Dory   "..pair.answer
        for _, msg in ipairs(messages) do
            local _, wrapped = g.getFont():getWrap(msg, w-40)
            for _, line in ipairs(wrapped) do lines[#lines+1] = line end
        end
    end
    local count = math.max(1, math.floor((keyTop()-148)/28))
    scroll = math.min(scroll, math.max(0,#lines-count))
    local first = math.max(1, #lines-scroll-count+1)
    for i = first, math.min(#lines,first+count-1) do g.print(lines[i],18,52+(i-first)*28) end
    local _, wrapped = g.getFont():getWrap(draft,w-40)
    g.print(wrapped[#wrapped] or "",18,keyTop()-68)
    g.print(status ~= "" and status or "D-pad select   A type   + send   L/R scroll",18,keyTop()-36)
    for r, group in ipairs(keys) do
        for c, k in ipairs(group) do
            local width, y = w/#group, keyTop()+(r-1)*44
            if row == r and col == c then g.setColor(0.12,0.48,0.55) else g.setColor(0.15,0.2,0.26) end
            g.rectangle("fill",(c-1)*width+2,y,width-4,40,5,5)
            g.setColor(1,1,1)
            g.printf(shift and #k == 1 and k:upper() or k,(c-1)*width,y+6,width,"center")
        end
    end
end
