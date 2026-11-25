# Switch 2 agent and session laws

From the Rillwright repository root, run both checks.

```sh
bend ports/switch2/laws/PROOF.bend --verdict
luajit ports/switch2/laws/check_app.lua
```

The Bend verdict checks all ten claims in `LAWS.bend`. The write decision assumes that the caller supplies the user's approval and the result of comparing the file with its last observed contents. The session-control decision models the guards on starting a turn or activating a session control. The round budget counts previously dispatched calls.

The two framing proofs use induction over the payload. Once the RW1 reader has parsed a field length, taking that many elements recovers the complete field and dropping them preserves the entire suffix. Elements are modeled as U32 values, which include every byte value.

The Lua check drives the port's input callbacks, tool execution, and RW1 reader and writer. It supplies an in-memory filesystem and scripted worker replies. This exercises the connection to the Lua implementation, including its decimal length parser. The kernel verdict applies to the Bend definitions. Console input delivery, SD filesystem durability, and Dory inference are outside its scope.
