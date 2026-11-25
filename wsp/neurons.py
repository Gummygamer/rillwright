"""Direct attribution of output-block MLP neurons to the </think> logit, from recorded states."""
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gg import Model
import think_stats as T
T.PROBE = "target/wsp/probe_all"
m = T.model(); TH = 13
STD = float(m.kv("dory.init_std")); AINIT = float(m.kv("dory.alpha_init")); D = 2560
names = ['calc-s1-r2','calc-s2-r2','lru-s1-r3','lru-s1-r0','lru-s1-r5','calc-s1-r0','calc-s2-r0']
def load(n):
    j = json.load(open(f"{T.PROBE}/{n}.json"))
    h = np.memmap(f"{T.PROBE}/{n}.f32", dtype=np.float32, mode="r").reshape(j["steps"], j["layers"], D)
    text = [f[2] for f in j["forced"] if f]
    return h, text
def norm(x): return x / np.linalg.norm(x, axis=-1, keepdims=True)
def ffn(p, x):
    """activations a (n, 7168) and output norm for physical ffn layer p on inputs x (n, 2560)"""
    Wg = m.get(f"blk.{p}.ffn_gate.weight"); Wu = m.get(f"blk.{p}.ffn_up.weight")
    sg = m.get(f"blk.{p}.ffn_gate_scale.weight") * np.sqrt(D); su = m.get(f"blk.{p}.ffn_up_scale.weight") * np.sqrt(D)
    g = (x @ Wg.T) * sg; u = (x @ Wu.T) * su
    a = norm(g / (1 + np.exp(-g)) * u)
    return a
sel = {}
for n in names:
    h, text = load(n)
    lp, _ = T.logprobs(np.array(h[:, -1]), [TH]); lp = lp[TH]
    te = text.index('</think>') if '</think>' in text else len(text)
    sent = np.array([i for i in range(1, te + 1) if i == te or text[i-1].endswith(('.', '.\n', '.\n\n', '\n\n'))])
    sel[n] = (h, text, lp, te, sent)
groups = {"over": [], "cont": [], "close": []}   # (name, idx)
for n in names:
    h, text, lp, te, sent = sel[n]
    for i in sent:
        if lp[i] > -2: groups["close"].append((n, i))
        elif lp[i] < -8:
            if n.startswith('calc') and n.endswith('r2') and i > 400: groups["over"].append((n, i))
            elif not (n.startswith('calc') and n.endswith('r2')): groups["cont"].append((n, i))
print({k: len(v) for k, v in groups.items()})
w = T.W[TH]
res = {}
for il in range(97, 112, 2):
    p = 56 + (il - 96)
    Wd = m.get(f"blk.{p}.ffn_down.weight")          # (2560, 7168)
    alpha = np.abs(m.get(f"blk.{p}.residual_alpha.weight")) * AINIT / STD
    dirw = (alpha * w) @ Wd                          # per-neuron push on the </think> logit direction
    row = {}
    for g, items in groups.items():
        X = np.stack([np.array(sel[n][0][i, il - 1]) for n, i in items])
        a = ffn(p, X)
        onorm = np.linalg.norm(a @ Wd.T, axis=1, keepdims=True)
        row[g] = (a * dirw[None, :] / onorm)         # (n, 7168) contribution
    res[il] = row
    mo, mc, mk = row["over"].mean(0), row["cont"].mean(0), row["close"].mean(0)
    print(f"il {il} p {p}: total push  over {mo.sum():+.4f}  cont {mc.sum():+.4f}  close {mk.sum():+.4f} | most negative in over:",
          [(int(j), round(float(mo[j]), 4), round(float(mc[j]), 4), round(float(mk[j]), 4)) for j in np.argsort(mo)[:6]])
    order = np.argsort(mo)
    for k in (8, 32, 64, 256):
        print(f"      top-{k} suppressors sum: over {mo[order[:k]].sum():+.4f} cont {mc[order[:k]].sum():+.4f} close {mk[order[:k]].sum():+.4f}")
np.save("target/wsp/neuron_push.npy", {il: {g: v.mean(0) for g, v in r.items()} for il, r in res.items()}, allow_pickle=True)
print("gain per unit of push on the logit:", T.SCALE[TH] / STD)
