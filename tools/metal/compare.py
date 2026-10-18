"""Compares two folders of same-named frame dumps (tools/metal/compare.sh): per frame, the share of byte-identical
pixels and of pixels whose channels all differ by at most 2."""
import os
import sys

from PIL import Image


def main(a_dir, b_dir):
    a_names, b_names = set(os.listdir(a_dir)), set(os.listdir(b_dir))
    if a_names ^ b_names:
        print("only on one side:", sorted(a_names ^ b_names)[:10], len(a_names ^ b_names))
    rows = []
    for n in sorted(a_names & b_names):
        a = Image.open(os.path.join(a_dir, n)).convert("RGB")
        b = Image.open(os.path.join(b_dir, n)).convert("RGB")
        if a.size != b.size:
            print(f"  {n}: sizes differ ({a.size} vs {b.size})")
            continue
        pa, pb = a.tobytes(), b.tobytes()
        total = len(pa) // 3
        same = near = 0
        for i in range(0, len(pa), 3):
            d = max(abs(pa[i] - pb[i]), abs(pa[i + 1] - pb[i + 1]), abs(pa[i + 2] - pb[i + 2]))
            same += d == 0
            near += d <= 2
        rows.append((n, 100 * same / total, 100 * near / total))
    if not rows:
        print("no frames to compare")
        return 1
    print(
        f"{len(rows)} frames; exact: mean {sum(r[1] for r in rows) / len(rows):.3f}%, min {min(r[1] for r in rows):.3f}%; "
        f"within 2: mean {sum(r[2] for r in rows) / len(rows):.3f}%, min {min(r[2] for r in rows):.3f}%"
    )
    for n, same, near in sorted(rows, key=lambda r: r[2])[:8]:
        print(f"  {n}: exact {same:.3f}%, within 2 {near:.3f}%")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: compare.py <dir a> <dir b>")
    sys.exit(main(sys.argv[1], sys.argv[2]))
