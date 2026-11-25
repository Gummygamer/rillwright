"""Read Dory weights from the GGUF file and write a copy with some tensors replaced.

Run with target/wsp/venv/bin/python (numpy and the fork's gguf-py).

    m = Model()                      # RILLWRIGHT_MODEL / DORY_MODEL
    W = m.get("blk.41.ffn_down.weight")   # float32, shape (out, in): numpy order is the reverse of ggml's
    m.write(out_path, {"blk.41.ffn_down.weight": W_new})

A replaced tensor is stored as F32 with exactly the values given; every other tensor is copied
byte for byte. The values read are the dequantized ones, which are what the runtime computes with.
"""
import hashlib
import json
import os

import numpy as np
import gguf
from gguf import GGUFReader, GGUFWriter, GGMLQuantizationType
from gguf.quants import dequantize


def model_path():
    return os.environ.get("RILLWRIGHT_MODEL") or os.environ["DORY_MODEL"]


class Model:
    def __init__(self, path=None):
        self.path = path or model_path()
        self.reader = GGUFReader(self.path)
        self.tensors = {t.name: t for t in self.reader.tensors}
        self._cache = {}

    def kv(self, key):
        f = self.reader.get_field(key)
        return f.contents() if f else None

    def get(self, name, cache=False):
        if name in self._cache:
            return self._cache[name]
        t = self.tensors[name]
        if t.tensor_type in (GGMLQuantizationType.F32,):
            a = np.array(t.data, dtype=np.float32)
        else:
            a = dequantize(t.data, t.tensor_type).astype(np.float32)
        a = a.reshape([int(x) for x in reversed(t.shape)])
        if cache:
            self._cache[name] = a
        return a

    def write(self, out_path, edits, note=None):
        """Copies the model to out_path with `edits` (name -> float32 array) replacing tensors."""
        r = self.reader
        arch = self.kv("general.architecture")
        w = GGUFWriter(out_path, arch=arch, endianess=r.endianess)
        for field in r.fields.values():
            if field.name == gguf.Keys.General.ARCHITECTURE or field.name.startswith("GGUF."):
                continue
            vt = field.types[0]
            sub = field.types[-1] if vt == gguf.GGUFValueType.ARRAY else None
            w.add_key_value(field.name, field.contents(), vt, sub_type=sub)
        if note:
            w.add_key_value("wsp.edit_note", note, gguf.GGUFValueType.STRING)
        new = {}
        for t in r.tensors:
            if t.name in edits:
                a = np.ascontiguousarray(edits[t.name], dtype=np.float32)
                want = tuple(int(x) for x in reversed(t.shape))
                assert a.shape == want, (t.name, a.shape, want)
                assert np.isfinite(a).all(), t.name
                new[t.name] = a
                w.add_tensor_info(t.name, a.shape, a.dtype, a.nbytes, GGMLQuantizationType.F32)
            else:
                w.add_tensor_info(t.name, t.data.shape, t.data.dtype, t.data.nbytes, t.tensor_type)
        missing = set(edits) - set(new)
        assert not missing, f"no such tensors: {missing}"
        w.write_header_to_file()
        w.write_kv_data_to_file()
        w.write_ti_data_to_file()
        for t in r.tensors:
            w.write_tensor_data(new[t.name] if t.name in new else t.data, tensor_endianess=r.endianess)
        w.close()
        h = hashlib.sha256()
        with open(out_path, "rb") as f:
            while chunk := f.read(8 << 20):
                h.update(chunk)
        with open(out_path + ".sha256", "w") as f:
            f.write(h.hexdigest() + "\n")
        return h.hexdigest()


def manifest(m, edits, path, extra=None):
    """Records what an edit changed: per tensor, how many values differ and by how much."""
    out = dict(base=os.path.basename(m.path), tensors={}, **(extra or {}))
    total = 0
    for name, new in edits.items():
        old = m.get(name)
        d = new.astype(np.float32) - old
        changed = int((d != 0).sum())
        total += changed
        out["tensors"][name] = dict(shape=list(old.shape), changed=changed, max_abs_delta=float(np.abs(d).max()),
                                    rel_fro=float(np.linalg.norm(d) / (np.linalg.norm(old) + 1e-30)))
    out["changed_values"] = total
    with open(path, "w") as f:
        json.dump(out, f, indent=1)
    return out
