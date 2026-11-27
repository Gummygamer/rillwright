package.path="ports/switch2/?.lua;"..package.path
local data,dirs={},{}
local fs={}
love={filesystem=fs,graphics={newFont=function() end,setFont=function() end},thread={}}
function fs.getSaveDirectory() return "/mock" end
function fs.createDirectory(p) dirs[p]=true; return true end
function fs.write(p,s) data[p]=s; return true end
function fs.read(p) return data[p] end
function fs.getInfo(p)
 if data[p] then return {type="file",size=#data[p]} elseif dirs[p] then return {type="directory"} end
end
function fs.getDirectoryItems(p)
 local a={}
 for k in pairs(data) do if k:sub(1,#p+1)==p.."/" and not k:sub(#p+2):find("/") then a[#a+1]=k:sub(#p+2) end end
 return a
end
function os.rename(a,b) a,b=a:sub(7),b:sub(7);data[b],data[a]=assert(data[a]),nil;return true end
local reply,prompt
function love.thread.newThread() return {start=function(_,s) prompt=s end,getError=function()end} end
local pieces={}
local channels={
 reply={pop=function() local r=reply;reply=nil;return r end},
 stream={pop=function() return table.remove(pieces,1) end,clear=function() pieces={} end}
}
function love.thread.getChannel(name) return channels[name] end
local function output(s) reply={ok=true,text=s};love.update() end
dofile("ports/switch2/main.lua")
love.load()
love.textinput("Save a greeting");love.keypressed("return")
assert(prompt:find("write_file",1,true))
output("<tool_call><function=write_file><parameter=path>hello.txt</parameter><parameter=content>Hello</parameter></function></tool_call>")
assert(not data["workspace/hello.txt"])
love.gamepadpressed(nil,"a");assert(data["workspace/hello.txt"]=="Hello")
assert(prompt:find("Saved hello.txt",1,true));output("Greeting saved.")
local store=require("storage")
local entries=store.load(assert(store.sessions()[1]))
assert(entries[1].user=="Save a greeting" and entries[1].trace:find("write_file") and entries[1].answer=="Greeting saved.")
dofile("ports/switch2/main.lua");love.load()
love.textinput("read it");love.keypressed("return");assert(prompt:find("Greeting saved.",1,true))
local tools=require("tools");local seen={}
assert(tools.run("read_file",{path="hello.txt"},seen)=="Hello")
assert(tools.run("edit_file",{path="hello.txt",old="Hello",new="Hi"},seen))
assert(tools.run("search_files",{query="Hi"},seen)=="hello.txt")
assert(tools.run("list_dir",{},seen)=="hello.txt")
assert(not pcall(tools.run,"read_file",{path="../sessions/current"},seen))
assert(not pcall(tools.run,"write_file",{path="hello.txt",content="bad"},{}))
output("<tool_call><function=write_file><parameter=path>x</parameter><parameter=content>bad</parameter></function></tool_call>")
love.gamepadpressed(nil,"b");assert(prompt:find("User declined",1,true));assert(not data["workspace/x"])
output("Declined.")
assert(#store.load(store.sessions()[1])==2)

-- A session control cannot interrupt a turn, including while approval is pending.
local function key(k) love.gamepadpressed(nil,k) end
local function send(s) love.textinput(s); love.keypressed("return") end
local function selectNew()
    for i=1,3 do key("dpdown") end
    for i=1,4 do key("dpright") end
    key("a")
end
send("List the files")
local active = data["sessions/current"]
selectNew()
assert(data["sessions/current"] == active and #store.sessions() == 1)
local listing="<tool_call><function=list_dir></function></tool_call>"
for i=1,6 do
    prompt=nil
    output(listing)
    assert(prompt and prompt:find("hello.txt",1,true), "Expected tool result at round "..i)
end
prompt=nil
output(listing)
assert(prompt==nil and store.load(active)[3].answer=="Tool round limit reached.")
key("a") -- New, now idle
assert(#store.sessions()==2 and data["sessions/current"]~=active)
key("dpright"); key("a") -- Sessions
prompt=nil
send("Ignored while picker is open")
assert(prompt==nil)
assert(#store.sessions()==2)

-- Exercise the actual RW1 decimal framing, including embedded separators,
-- NUL bytes, empty fields, and a length prefix spanning multiple digits.
local binary=":\n\0" .. string.rep("x",127)
local fixture=store.new()
store.save(fixture,{{user="",answer=binary,trace=":"},{user=binary,answer="",trace="\n"}})
local restored=store.load(fixture)
assert(#restored==2 and restored[1].user=="" and restored[1].answer==binary)
assert(restored[1].trace==":" and restored[2].user==binary and restored[2].answer=="" and restored[2].trace=="\n")
local path="sessions/"..fixture..".chat"
data[path]=data[path]:sub(1,-2)
assert(not pcall(store.load,fixture))
-- Streamed pieces render as reasoning, a partial tool call, then the reply.
local ui=require("ui")
local captured
ui.draw=function(state) captured=state.live; return state.scroll,state.approvalScroll end
local function stream(...) for _,p in ipairs({...}) do pieces[#pieces+1]=p end; love.update(); love.draw() end
local function blocks() local a={} for _,b in ipairs(captured.blocks) do a[#a+1]=b.label.."="..b.text end return table.concat(a,"|") end
key("b") -- close the session picker
send("What is in hello.txt?")
assert(prompt:sub(-30)=="<|im_start|>assistant\n<think>\n")
stream("I should ","read it.")
assert(blocks()=="THINKING …=I should read it.")
stream("</think>\n<tool_call><function=read_file><parameter=path>hel")
assert(blocks()=="THINKING=I should read it.|TOOL CALL …=read_file\npath: hel")
stream("\xC3")
assert(blocks():sub(-3)=="hel", "partial UTF-8 is held back")
stream(false, "Retried")
assert(blocks()=="THINKING …=Retried")
output("I should read it.</think>\n<tool_call><function=read_file><parameter=path>hello.txt</parameter></function></tool_call>")
love.draw()
assert(blocks()=="THINKING=I should read it.|TOOL CALL=read_file\npath: hello.txt|TOOL RESULT=Hi")
stream("Done.</think>It says ","Hi.")
assert(blocks():find("|DORY …=It says Hi.",1,true))
output("Done.</think>It says Hi.")
love.draw()
assert(captured==nil and store.load(data["sessions/current"])[1].answer=="It says Hi.")
print("Rillwright Lua behavior checks passed")
