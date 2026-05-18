"""Pure-Python reference implementation for Rusty Red PPR tests."""

from __future__ import annotations

from collections import deque
from typing import Dict, List, Tuple

Adjacency = Dict[int, List[Tuple[int, float]]]
Seeds = Dict[int, float]


def python_push_ppr(
    adjacency: Adjacency,
    seeds: Seeds,
    *,
    alpha: float = 0.15,
    epsilon: float = 1e-4,
    max_pushes: int = 200_000,
) -> Dict[int, float]:
    residual = dict(seeds)
    ppr: Dict[int, float] = {}
    queued = set()
    queue = deque()

    def out_weight(node: int) -> float:
        return sum(weight for _, weight in adjacency.get(node, []))

    def threshold(node: int) -> float:
        return epsilon * max(out_weight(node), 1.0)

    for node in seeds:
        if residual.get(node, 0.0) > threshold(node):
            queue.append(node)
            queued.add(node)

    pushes = 0
    while queue and pushes < max_pushes:
        node = queue.popleft()
        queued.discard(node)
        mass = residual.get(node, 0.0)
        if mass <= threshold(node):
            continue

        ppr[node] = ppr.get(node, 0.0) + alpha * mass
        residual[node] = 0.0
        remaining = (1.0 - alpha) * mass
        weight_total = out_weight(node)
        if weight_total <= 0.0:
            pushes += 1
            continue

        for neighbor, weight in adjacency.get(node, []):
            residual[neighbor] = residual.get(neighbor, 0.0) + remaining * (
                weight / weight_total
            )
            if residual[neighbor] > threshold(neighbor) and neighbor not in queued:
                queue.append(neighbor)
                queued.add(neighbor)
        pushes += 1

    return ppr
