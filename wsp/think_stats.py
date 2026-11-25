"""Reads probe output and reports how willing the model is to stop deliberating at each position.

For every probed position it computes the full next-token distribution from the final-layer state
h and the output weights: logit_t = scale_t / init_std * (W_t . h). Optionally applies a candidate
edit to chosen rows of W first, so an edit can be tried on recorded states without running the model.
"""
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gg import Model

PROBE = "target/wsp/probe"
_m = None
def model():
    global _m, W, SCALE, STD
    if _m is None:
        _m = Model()
        W = _m.get("output.weight")            # (vocab, 2560)
        SCALE = _m.get("output_scale.weight")  # (vocab,)
        STD = float(_m.kv("dory.init_std"))
    return _m

def load(name):
    j = json.load(open(f"{PROBE}/{name}.json"))
    h = np.fromfile(f"{PROBE}/{name}.f32", dtype=np.float32).reshape(j["steps"], -1)
    toks = [f[0] for f in j["forced"] if f]
    text = [f[2] for f in j["forced"] if f]
    return j, h, toks, text

def logprobs(h, ids, delta=None):
    """log p(id) at every position for each id in ids. delta: {id: vector added to W[id]}."""
    model()
    logits = (h @ W.T) * (SCALE / STD)[None, :]
    if delta:
        for t, d in delta.items():
            logits[:, t] += (h @ d) * SCALE[t] / STD
    mx = logits.max(1, keepdims=True)
    lse = mx[:, 0] + np.log(np.exp(logits - mx).sum(1))
    return {t: logits[:, t] - lse for t in ids}, logits.argmax(1)
