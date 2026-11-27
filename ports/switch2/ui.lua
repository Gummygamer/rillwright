local ui = {}
local g, fonts, hits
local transcript = {}
local streamed = {}
local lastTotal, lastWidth
local review = {}
local shown
local width, height = 1280, 720
local colors = {
    background = {0.035, 0.055, 0.075},
    panel = {0.072, 0.105, 0.137},
    field = {0.041, 0.067, 0.092},
    key = {0.125, 0.176, 0.222},
    line = {0.19, 0.26, 0.31},
    text = {0.94, 0.97, 0.99},
    muted = {0.66, 0.74, 0.80},
    mint = {0.54, 0.95, 0.80},
    ink = {0.035, 0.14, 0.13},
    amber = {1, 0.77, 0.48}
}

local function viewport()
    local w, h = g.getWidth(), g.getHeight()
    local scale = math.min(w / width, h / height)
    return (w - width * scale) / 2, (h - height * scale) / 2, scale
end

local function box(x, y, w, h, color, radius)
    g.setColor(colors[color])
    g.rectangle("fill", x, y, w, h, radius or 12)
end

local function label(value, x, y, font, color, w, align)
    g.setColor(colors[color or "text"])
    g.setFont(fonts[font])
    if w then g.printf(value, x, y, w, align or "left")
    else g.print(value, x, y) end
end

local function button(value, x, y, w, h, action, fill, ink, row, col)
    box(x, y, w, h, fill or "key", 8)
    label(value, x, y + (h - fonts.small:getHeight()) / 2, "small", ink, w, "center")
    if action then hits[#hits + 1] = {x=x, y=y, w=w, h=h, action=action, row=row, col=col} end
end

function ui.hit(x, y, screen)
    if not hits or shown ~= screen then return end
    local ox, oy, scale = viewport()
    x, y = (x - ox) / scale, (y - oy) / scale
    for _, hit in ipairs(hits) do
        if x >= hit.x and x < hit.x + hit.w and y >= hit.y and y < hit.y + hit.h then
            return hit.action, hit.row, hit.col
        end
    end
end

local function wrap(value, w, font)
    local _, lines = fonts[font]:getWrap(value, w)
    return lines
end

local function message(lines, w, role, value, color, ink)
    lines[#lines + 1] = {value=role, font="small", color=color}
    for _, line in ipairs(wrap(value, w, "body")) do
        lines[#lines + 1] = {value=line, font="body", color=ink or "text"}
    end
    lines[#lines + 1] = {value="", font="small", color="text"}
end

local function historyLines(history, w)
    if transcript.history == history and transcript.count == #history and transcript.width == w then
        return transcript.lines
    end
    local lines = {}
    for _, pair in ipairs(history) do
        message(lines, w, "YOU", pair.user, "muted")
        if pair.trace and pair.trace ~= "" then message(lines, w, "TOOL ACTIVITY", pair.trace, "amber") end
        message(lines, w, "DORY", pair.answer, "mint")
    end
    transcript = {history=history, count=#history, width=w, lines=lines}
    return lines
end

-- History followed by the turn being generated: reasoning, tool calls and
-- their results, then the reply, each growing as pieces stream in.
local function conversationLines(history, live, w)
    local lines = historyLines(history, w)
    if not live then streamed = {}; return lines end
    if streamed.live ~= live or streamed.base ~= lines then
        local all = {}
        for i, line in ipairs(lines) do all[i] = line end
        message(all, w, "YOU", live.user, "muted")
        for _, block in ipairs(live.blocks) do
            message(all, w, block.label, block.text, block.color, block.color == "muted" and "muted" or nil)
        end
        streamed = {live=live, base=lines, lines=all}
    end
    return streamed.lines
end

local function conversation(state, w)
    box(32, 108, w, 544, "panel")
    label("Conversation", 56, 129, "heading")
    local lines = conversationLines(state.history, state.live, w - 48)
    local visible = 17
    local scroll = state.scroll
    -- While scrolled back, hold the view still as streamed lines arrive.
    if scroll > 0 and lastWidth == w and lastTotal and #lines > lastTotal then
        scroll = scroll + #lines - lastTotal
    end
    lastTotal, lastWidth = #lines, w
    scroll = math.min(scroll, math.max(0, #lines - visible))
    if #lines == 0 then
        label("Start a conversation with Dory", 56, 264, "heading", "text", w - 48, "center")
    else
        local last = #lines - scroll
        local first = math.max(1, last - visible + 1)
        -- Keep a role label and its first line together at the bottom edge.
        if last < #lines and lines[last].value ~= "" and lines[last].font == "small" then
            last = last - 1
        end
        for i = first, last do
            local line = lines[i]
            label(line.value, 56, 184 + (i - first) * 26, line.font, line.color)
        end
        if #lines > visible then
            button("L  Older", w - 208, 119, 105, 42, "older")
            button("R  Newer", w - 95, 119, 105, 42, "newer")
            local trackHeight = visible * 26
            local barHeight = math.max(20, trackHeight * visible / #lines)
            local progress = 1 - scroll / (#lines - visible)
            box(w + 19, 184 + (trackHeight - barHeight) * progress, 4, barHeight, "mint", 2)
        end
    end
    return scroll
end

local function composer(state)
    local x, w = 684, 564
    box(x, 108, w, 544, "panel")
    label("Message", x + 24, 129, "heading")
    label(#state.draft .. " / 2000 bytes", x + 270, 136, "small", "muted", 270, "right")
    box(x + 24, 178, w - 48, 120, "field")
    if state.draft == "" then
        label("Type a message…", x + 40, 195, "body", "muted")
    else
        local lines = wrap(state.draft, w - 80, "body")
        local first = math.max(1, #lines - 3)
        for i = first, #lines do
            local line = lines[i]
            if i == first and first > 1 then line = "…" .. line end
            label(line, x + 40, 187 + 26 * (i - first), "body")
        end
    end
    local status = state.status
    if status ~= "" then
        local lines = wrap(status, w - 48, "small")
        for i = 1, math.min(2, #lines) do
            label(lines[i], x + 24, 308 + (i - 1) * 21, "small", state.active and "mint" or "amber")
        end
    end
    for r, group in ipairs(state.keys) do
        local step = (w - 48 + 6) / #group
        for c, key in ipairs(group) do
            local selected = r == state.row and c == state.col
            local value = state.shift and #key == 1 and key:upper() or key
            local fill = selected and "mint" or (key == "Send" and "line" or "key")
            local ink = selected and "ink" or (state.active and "muted" or "text")
            if key == "Shift" and state.shift and not selected then ink = "mint" end
            button(value, x + 24 + (c - 1) * step, 366 + (r - 1) * 62,
                step - 6, 54, not state.active and "key" or nil, fill, ink, r, c)
        end
    end
end

local function dialog(state)
    box(180, 108, 920, 544, "panel")
    local offset = state.approvalScroll
    if state.menu then
        local menu = state.menu
        label("Saved sessions", 208, 132, "heading")
        label(menu.index .. " of " .. #menu.ids, 745, 139, "small", "muted", 327, "right")
        local first = math.max(1, menu.index - 6)
        for i = first, math.min(#menu.ids, first + 6) do
            local selected = menu.index == i
            local y = 190 + (i - first) * 49
            box(208, y, 864, 42, selected and "mint" or "field", 8)
            label(menu.ids[i], 224, y + 8, "body", selected and "ink" or "text")
            if menu.ids[i] == state.id then
                label("Current session", 800, y + 11, "small", selected and "ink" or "muted", 256, "right")
            end
        end
        button("Up", 208, 582, 138, 48, "dpup")
        button("Down", 354, 582, 138, 48, "dpdown")
        button("B  Back", 776, 582, 138, 48, "b")
        button("A  Open", 922, 582, 150, 48, "a", "mint", "ink")
    else
        local call = state.pending
        label("Review file change", 208, 132, "heading", "amber")
        label(call.name == "edit_file" and "Replace text in workspace" or "Write workspace file",
            208, 172, "small", "muted")
        box(208, 211, 864, 350, "field")
        if review.call ~= call then
            local args = call.args
            local details = "Path: " .. args.path
            if call.name == "edit_file" then
                details = details .. "\n\nFind\n" .. args.old .. "\n\nReplace with\n" .. args.new
            else
                details = details .. "\n\nContents\n" .. args.content
            end
            review = {call=call, lines=wrap(details, 832, "body")}
        end
        local lines = review.lines
        local count = 12
        offset = math.min(offset, math.max(0, #lines - count))
        for i = offset + 1, math.min(#lines, offset + count) do
            label(lines[i], 224, 228 + (i - offset - 1) * 26, "body")
        end
        button("L  Up", 208, 582, 138, 48, "leftshoulder")
        button("R  Down", 354, 582, 138, 48, "rightshoulder")
        button("B  Decline", 776, 582, 138, 48, "b")
        button("A  Approve", 922, 582, 150, 48, "a", "mint", "ink")
    end
    return offset
end

function ui.draw(state)
    g = love.graphics
    if not fonts then
        fonts = {title=g.newFont(32), heading=g.newFont(25), body=g.newFont(20), small=g.newFont(17)}
    end
    hits = {}
    shown = state.pending or state.menu or state.reading
    if not state.pending then review = {} end
    local ox, oy, scale = viewport()
    g.clear(colors.background)
    g.push()
    g.translate(ox, oy)
    g.scale(scale)
    box(32, 36, 10, 34, "mint", 4)
    label("Rillwright", 56, 31, "title")
    label("DORY", 236, 42, "small", "mint")
    box(32, 88, 1216, 2, "line", 0)
    local scroll, offset = state.scroll, state.approvalScroll
    if state.menu or state.pending then
        offset = dialog(state)
        if state.menu then label("D-pad  Select session", 32, 675, "small", "muted") end
    else
        button(state.reading and "X  Show keyboard" or "X  Reading view",
            1036, 30, 212, 44, "view")
        scroll = conversation(state, state.reading and 1216 or 628)
        if not state.reading then composer(state)
        elseif state.status ~= "" then
            local lines = wrap(state.status, 1168, "small")
            for i = 1, math.min(2, #lines) do
                label(lines[i], 56, 626 + (i - 1) * 21, "small", state.active and "mint" or "amber")
            end
        end
        local help = state.reading and "D-pad / L / R  Scroll     X  Show keyboard"
            or "D-pad  Select     A  Type     B  Delete     +  Send     L / R  Scroll"
        label(help, 32, 675, "small", "muted")
        local session = state.id and "Session " .. state.id or "New conversation"
        label(session, 826, 675, "small", "muted", 422, "right")
    end
    g.pop()
    return scroll, offset
end

return ui
