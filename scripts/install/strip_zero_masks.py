#!/usr/bin/env python3
"""Drop the all-zero attention masks the TableFormer encoder export bakes in (#374).

`export_tableformer.py` calls the tag-transformer encoder with the explicit
`mask=torch.zeros((heads, 784, 784), dtype=torch.bool)` docling's module
expects. The legacy ONNX exporter turns that bool mask into the additive
float mask nn.MultiheadAttention adds to the attention scores — a constant
`[1, 8, 784, 784]` fp32 tensor of zeros, 18.8 MiB, materialized once per
encoder layer. Six layers → 112.6 MiB of `x + 0` in a 215 MiB file whose real
weights are 103 MiB (42.6 MiB of ResNet convs, 60 MiB of transformer
MatMul/Gemm).

Adding a zero tensor is the identity, so removing those `Add` nodes and
re-wiring their consumers to the other operand is exact: the stripped graph's
outputs are bit-identical to the original's (checked with onnxruntime on the
published models-v1 encoder, max |diff| = 0). The mask stays explicit in the
export itself so the PyTorch module runs the same code path docling runs
(`mask=None` could route nn.TransformerEncoder onto its fused fast path and
change the numerics the byte-exact OTSL verification depends on).

Usage — in place, or into a second file:

    python scripts/install/strip_zero_masks.py .models/tableformer/encoder.onnx [out.onnx]

Only `Add` nodes whose one operand is a *constant that is entirely zero* and at
least 1 MiB are touched; anything else in the graph is left as it is. The
script refuses (non-zero exit) when such an operand turns out not to be
all-zero, so a future export that carries a real mask cannot be silently
broken.
"""
import os
import sys

import numpy as np
import onnx
from onnx import numpy_helper

MIN_BYTES = 1 << 20  # only the mask-sized constants; small biases stay


def strip_zero_mask_adds(src, dst=None):
    """Remove `Add(x, zeros)` nodes fed by a large all-zero constant; returns
    (nodes removed, bytes freed). Saves to `dst` (default: over `src`)."""
    dst = dst or src
    model = onnx.load(src)
    g = model.graph
    const_nodes = {n.output[0]: n for n in g.node if n.op_type == "Constant"}
    inits = {t.name: t for t in g.initializer}

    def constant_value(name):
        if name in inits:
            return numpy_helper.to_array(inits[name])
        node = const_nodes.get(name)
        if node is None:
            return None
        for attr in node.attribute:
            if attr.name == "value":
                return numpy_helper.to_array(attr.t)
        return None

    rewired = {}
    kept = []
    removed = freed = 0
    for n in g.node:
        if n.op_type == "Add" and len(n.input) == 2:
            big = None
            for i in n.input:
                v = constant_value(i)
                if v is not None and v.nbytes >= MIN_BYTES:
                    big = (i, v)
                    break
            if big is not None:
                name, v = big
                if v.dtype.kind not in "fiu" or v.any():
                    raise SystemExit(
                        f"{src}: Add operand {name} ({v.shape}, {v.dtype}) is not "
                        "all-zero — refusing to strip a real mask"
                    )
                other = [i for i in n.input if i != name][0]
                rewired[n.output[0]] = other
                removed += 1
                freed += v.nbytes
                continue
        kept.append(n)
    # Re-wire consumers (chains resolve transitively) and drop the now-dead
    # constants; graph outputs that pointed at a removed Add keep their names
    # through an Identity so the exported signature does not change.
    for n in kept:
        for k, i in enumerate(n.input):
            while i in rewired:
                i = rewired[i]
            n.input[k] = i
    for out in g.output:
        if out.name in rewired:
            src_name = out.name
            while src_name in rewired:
                src_name = rewired[src_name]
            kept.append(onnx.helper.make_node("Identity", [src_name], [out.name]))
    used = {i for n in kept for i in n.input} | {o.name for o in g.output}
    kept = [n for n in kept if n.op_type != "Constant" or n.output[0] in used]
    for t in [t for t in g.initializer if t.name not in used]:
        g.initializer.remove(t)
    del g.node[:]
    g.node.extend(kept)
    onnx.checker.check_model(model)
    onnx.save(model, dst)
    return removed, freed


def main(argv):
    if len(argv) not in (2, 3):
        sys.exit(__doc__)
    src = argv[1]
    dst = argv[2] if len(argv) == 3 else None
    before = os.path.getsize(src)
    removed, freed = strip_zero_mask_adds(src, dst)
    after = os.path.getsize(dst or src)
    print(
        f"{os.path.basename(src)}: removed {removed} zero-mask Add node(s) "
        f"({freed / 2**20:.1f} MiB of constants); {before / 2**20:.1f} → {after / 2**20:.1f} MiB"
    )


if __name__ == "__main__":
    main(sys.argv)
