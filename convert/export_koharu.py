"""Export the Koharu models (PyTorch safetensors) to ONNX for LabelDesk.

LabelDesk writes this script into its private converter folder and runs it with its own Python
environment (never the system one). Nothing here is run by the app itself; it only produces
ONNX files the app then reads with ONNX Runtime, or converts further to ncnn with pnnx.

    python export_koharu.py layout --weights model.safetensors --config config.json \
        --resolution 1920 --out model.onnx
    python export_koharu.py textmask --weights model.safetensors --hi-sam-root Hi-SAM-main --out model.onnx

Both models use attention over many thousands of image patches. Exported as is, ONNX Runtime
would hold one attention table per layer in memory (tens of gigabytes for the layout model at
1920 px), so attention is computed in slices of queries instead: the same numbers, a fraction
of the memory. `--slice-mb` sets the size of one slice.
"""
from __future__ import annotations

import argparse
import json
import math
import sys
import time
from pathlib import Path

import torch
import torch.nn.functional as F

SLICE_BYTES = 256 * 1024 * 1024


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def sliced_attention(q: torch.Tensor, k: torch.Tensor, v: torch.Tensor, scale: float) -> torch.Tensor:
    """softmax(q k^T * scale) v, computed for a few queries at a time. q, k, v: [..., L, d]."""
    lq, lk = q.shape[-2], k.shape[-2]
    lead = 1
    for s in q.shape[:-2]:
        lead *= int(s)
    rows = max(16, int(SLICE_BYTES // max(1, lk * lead * 4)))
    if rows >= lq:
        return torch.softmax((q * scale) @ k.transpose(-2, -1), dim=-1) @ v
    parts = []
    for i in range(0, lq, rows):
        qi = q[..., i : i + rows, :]
        parts.append(torch.softmax((qi * scale) @ k.transpose(-2, -1), dim=-1) @ v)
    return torch.cat(parts, dim=-2)


def no_checkpoint() -> None:
    """Gradient checkpointing wrappers are training-only; call the function directly."""
    import torch.utils.checkpoint as cp

    cp.checkpoint = lambda fn, *args, **kwargs: fn(*args)


# ------------------------------------------------------------------------------------- layout


def patch_dinov2() -> int:
    from rfdetr.models.backbone import dinov2_with_windowed_attn as d

    def forward(self, hidden_states, output_attentions=False, *args, **kwargs):
        q = self.transpose_for_scores(self.query(hidden_states))
        k = self.transpose_for_scores(self.key(hidden_states))
        v = self.transpose_for_scores(self.value(hidden_states))
        ctx = sliced_attention(q, k, v, 1.0 / math.sqrt(self.attention_head_size))
        ctx = ctx.permute(0, 2, 1, 3).contiguous()
        ctx = ctx.view(ctx.size()[:-2] + (self.all_head_size,))
        return (ctx, None)

    n = 0
    for name in ("Dinov2WithRegistersSelfAttention", "Dinov2WithRegistersSdpaSelfAttention"):
        cls = getattr(d, name, None)
        if cls is not None:
            cls.forward = forward
            n += 1
    return n


def freeze_positions(net, res: int) -> int:
    """Resize the position table for `res` once, here (anti-aliased bicubic, as the model does),
    so the exported graph holds the finished table instead of a resize ONNX can't express."""
    import types

    from rfdetr.models.backbone import dinov2_with_windowed_attn as d

    n = 0
    for m in net.modules():
        if isinstance(m, d.WindowedDinov2WithRegistersEmbeddings):
            dim = m.position_embeddings.shape[-1]
            patches = (res // m.config.patch_size) ** 2
            with torch.no_grad():
                table = m.interpolate_pos_encoding(torch.zeros(1, patches + 1, dim), res, res).detach().clone()
            m.register_buffer("_frozen_pos", table, persistent=False)
            m.interpolate_pos_encoding = types.MethodType(lambda self, e, h, w: self._frozen_pos, m)
            n += 1
    return n


def export_layout(args) -> None:
    from safetensors.torch import load_file

    cfg = json.loads(Path(args.config).read_text(encoding="utf-8"))
    arch = cfg.get("architecture", "RFDETRSeg2XLarge")
    import rfdetr

    cls = getattr(rfdetr, arch, None)
    if cls is None:
        raise SystemExit(f"rfdetr has no model called {arch}")
    log(f"reading weights {args.weights}")
    state = load_file(args.weights, device="cpu")
    if any(k.startswith("model.") for k in state) and not any(k.startswith("backbone") for k in state):
        state = {k[len("model."):]: v for k, v in state.items() if k.startswith("model.")}
    classes = next((v.shape[0] for k, v in state.items() if k.endswith("class_embed.weight")), None)
    pos = next((v for k, v in state.items() if k.endswith("position_embeddings")), None)
    patch = 12
    trained = int(cfg.get("resolution", 1920))
    kwargs = {"pretrain_weights": None, "resolution": trained}
    if classes:
        kwargs["num_classes"] = int(classes) - 1
    if pos is not None:
        side = int(round(math.sqrt(pos.shape[1] - 1)))
        kwargs["positional_encoding_size"] = side
        log(f"position table {side}x{side} patches, {classes} class outputs")
    model = cls(**kwargs)
    net = model.model.model
    missing, unexpected = net.load_state_dict(state, strict=False)
    log(f"weights loaded: {len(state) - len(unexpected)} tensors used, {len(missing)} missing, {len(unexpected)} unused")
    if len(missing) > 0:
        log("missing: " + ", ".join(list(missing)[:12]))
    if len(missing) > 8:
        raise SystemExit("the weights do not fit this model; nothing was written")
    log(f"attention computed in slices ({patch_dinov2()} attention kinds patched)")
    res = int(args.resolution)
    if res % 24:
        raise SystemExit("the resolution must be a multiple of 24")
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    net.eval()
    if hasattr(net, "export"):
        net.export()
    log(f"position table resized for {res} px ({freeze_positions(net, res)} table)")
    x = torch.rand(1, 3, res, res)
    log(f"tracing at {res}x{res} (this takes a while on a slow computer)")

    class Wrap(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, x):
            o = self.m(x)
            if isinstance(o, dict):
                return o["pred_boxes"], o["pred_logits"], o["pred_masks"]
            return o

    with torch.no_grad():
        torch.onnx.export(Wrap(net), (x,), str(out), input_names=["input"], output_names=["dets", "labels", "masks"], opset_version=17, do_constant_folding=True, dynamo=False)
    log(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB)")


# ---------------------------------------------------------------------------------- text mask


def patch_sam_attention() -> None:
    from hi_sam.modeling import image_encoder as ie

    def forward(self, x):
        B, H, W, _ = x.shape
        qkv = self.qkv(x).reshape(B, H * W, 3, self.num_heads, -1).permute(2, 0, 3, 1, 4)
        q, k, v = qkv.reshape(3, B * self.num_heads, H * W, -1).unbind(0)
        if H * W <= 1024:
            attn = (q * self.scale) @ k.transpose(-2, -1)
            if self.use_rel_pos:
                attn = ie.add_decomposed_rel_pos(attn, q, self.rel_pos_h, self.rel_pos_w, (H, W), (H, W))
            out = attn.softmax(dim=-1) @ v
        else:
            bh, _, dim = q.shape
            rh = ie.get_rel_pos(H, H, self.rel_pos_h) if self.use_rel_pos else None
            rw = ie.get_rel_pos(W, W, self.rel_pos_w) if self.use_rel_pos else None
            rows = max(1, int(SLICE_BYTES // max(1, H * W * W * bh * 4)))
            parts = []
            for r0 in range(0, H, rows):
                r1 = min(H, r0 + rows)
                qc = q[:, r0 * W : r1 * W]
                attn = (qc * self.scale) @ k.transpose(-2, -1)
                if self.use_rel_pos:
                    r_q = qc.reshape(bh, r1 - r0, W, dim)
                    rel_h = torch.einsum("bhwc,hkc->bhwk", r_q, rh[r0:r1])
                    rel_w = torch.einsum("bhwc,wkc->bhwk", r_q, rw)
                    attn = (attn.view(bh, r1 - r0, W, H, W) + rel_h[:, :, :, :, None] + rel_w[:, :, :, None, :]).view(bh, (r1 - r0) * W, H * W)
                parts.append(attn.softmax(dim=-1) @ v)
            out = torch.cat(parts, dim=1)
        x = out.view(B, self.num_heads, H, W, -1).permute(0, 2, 3, 1, 4).reshape(B, H, W, -1)
        return self.proj(x)

    ie.Attention.forward = forward


def export_textmask(args) -> None:
    from types import SimpleNamespace

    from safetensors.torch import load_file

    sys.path.insert(0, str(Path(args.hi_sam_root).resolve()))
    no_checkpoint()
    from hi_sam.modeling.build import model_registry

    patch_sam_attention()
    margs = SimpleNamespace(checkpoint=None, model_type="vit_l", attn_layers=1, prompt_len=12, hier_det=False)
    log(f"reading weights {args.weights}")
    state = load_file(args.weights, device="cpu")
    # build the model without allocating its own (random) weights, then take the file's
    # tensors as they are: one copy of 1.3 GB in memory instead of two
    try:
        with torch.device("meta"):
            model = model_registry["vit_l"](args=margs)
        model.load_state_dict(state, strict=True, assign=True)
        model.pixel_mean = torch.tensor([123.675, 116.28, 103.53]).view(-1, 1, 1)
        model.pixel_std = torch.tensor([58.395, 57.12, 57.375]).view(-1, 1, 1)
        left = [n for n, t in list(model.named_parameters()) + list(model.named_buffers()) if t.is_meta]
        if left:
            raise RuntimeError("tensors without weights: " + ", ".join(left[:5]))
        log("weights taken without a second copy")
    except Exception as e:  # older PyTorch or code that computes in its constructor
        log(f"building the model normally ({e})")
        model = model_registry["vit_l"](args=margs)
        model.load_state_dict(state, strict=True)
    del state
    model.eval()

    class Wrap(torch.nn.Module):
        """Raw RGB 0-255 page, resized so its long side is 1024 and padded with 128 -> text logits."""

        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, x):
            m = self.m
            x = (x - m.pixel_mean) / m.pixel_std
            emb = m.image_encoder(x)
            sparse = m.modal_aligner(emb)
            _, hr, _, _ = m.mask_decoder(image_embeddings=emb, image_pe=m.prompt_encoder.get_dense_pe(), sparse_prompt_embeddings=sparse, multimask_output=False)
            return F.interpolate(hr, (1024, 1024), mode="bilinear", align_corners=False)

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    x = torch.full((1, 3, 1024, 1024), 128.0)
    log("tracing at 1024x1024 (this takes a while on a slow computer)")
    with torch.no_grad():
        # no constant folding: it would copy the 1.3 GB of weights once more while exporting
        torch.onnx.export(Wrap(model), (x,), str(out), input_names=["image"], output_names=["text_logits"], opset_version=17, do_constant_folding=False, dynamo=False)
    log(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB)")


def main() -> None:
    global SLICE_BYTES
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("model", choices=["layout", "textmask"])
    p.add_argument("--weights", required=True)
    p.add_argument("--config")
    p.add_argument("--resolution", type=int, default=1920)
    p.add_argument("--hi-sam-root")
    p.add_argument("--out", required=True)
    p.add_argument("--slice-mb", type=int, default=256)
    a = p.parse_args()
    SLICE_BYTES = a.slice_mb * 1024 * 1024
    torch.set_num_threads(max(1, torch.get_num_threads()))
    log(f"PyTorch {torch.__version__}")
    # nn.MultiheadAttention's fused fast path has no ONNX form; the plain path computes the same
    try:
        torch.backends.mha.set_fastpath_enabled(False)
    except AttributeError:
        pass
    if a.model == "layout":
        if not a.config:
            raise SystemExit("--config is required for the layout model")
        export_layout(a)
    else:
        if not a.hi_sam_root:
            raise SystemExit("--hi-sam-root is required for the text mask model")
        export_textmask(a)
    log("done")


if __name__ == "__main__":
    main()
