require("love")
require("love.thread")
local prompt = ...
local stream = love.thread.getChannel("stream")
local ok, result = pcall(function()
    local model = require("ns2text")
    assert(model.ready(), "Dory model missing from switch/gen1recomp/models")
    assert(model.open())
    -- Runtimes without on_piece ignore it and only return the finished text.
    return assert(model.generate({prompt = prompt, max_tokens = 1024, on_piece = function(piece)
        stream:push(piece or false) -- false: discard pieces so far (CPU retry)
    end}))
end)
love.thread.getChannel("reply"):push({ok = ok, text = tostring(result)})
