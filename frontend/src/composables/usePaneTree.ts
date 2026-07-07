import type { PaneNode, SessionStatus } from '@/stores/session'

export function collectSessionIds(node: PaneNode): string[] {
  if (node.type === 'terminal') return [node.sessionId]
  return [...collectSessionIds(node.children[0]), ...collectSessionIds(node.children[1])]
}

export function findFirstTerminalId(node: PaneNode): string {
  if (node.type === 'terminal') return node.id
  return findFirstTerminalId(node.children[0])
}

export function removePaneFromTree(node: PaneNode, targetId: string): PaneNode | null {
  if (node.type === 'terminal') {
    return node.id === targetId ? null : node
  }
  if (node.children[0].type === 'terminal' && node.children[0].id === targetId) return node.children[1]
  if (node.children[1].type === 'terminal' && node.children[1].id === targetId) return node.children[0]
  const left = removePaneFromTree(node.children[0], targetId)
  if (left !== node.children[0]) {
    if (left === null) return node.children[1]
    node.children[0] = left as any
    return node
  }
  const right = removePaneFromTree(node.children[1], targetId)
  if (right !== node.children[1]) {
    if (right === null) return node.children[0]
    node.children[1] = right as any
    return node
  }
  return node
}

export function replacePaneWithSplit(
  node: PaneNode,
  targetId: string,
  direction: 'horizontal' | 'vertical',
  newPane: PaneNode
): boolean {
  if (node.type === 'split') {
    for (let i = 0; i < 2; i++) {
      const child = node.children[i]
      if (child.type === 'terminal' && child.id === targetId) {
        node.children[i] = {
          id: `split-${Date.now()}`,
          type: 'split',
          direction,
          children: [child, newPane],
          ratio: 0.5
        } as any
        return true
      }
      if (child.type === 'split' && replacePaneWithSplit(child, targetId, direction, newPane)) {
        return true
      }
    }
  }
  return false
}

export function updatePaneNodeId(node: PaneNode, pendingId: string, realId: string): boolean {
  if (node.type === 'terminal') {
    if (node.id === pendingId) {
      node.id = realId
      node.sessionId = realId
      return true
    }
    return false
  }
  return updatePaneNodeId(node.children[0], pendingId, realId) || updatePaneNodeId(node.children[1], pendingId, realId)
}

export function updateRatio(node: PaneNode, splitId: string, ratio: number): boolean {
  if (node.type === 'split') {
    if (node.id === splitId) {
      node.ratio = ratio
      return true
    }
    return updateRatio(node.children[0], splitId, ratio) || updateRatio(node.children[1], splitId, ratio)
  }
  return false
}

export interface PaneContext {
  getPaneTree: () => PaneNode | undefined
  setPaneTree: (tree: PaneNode | undefined) => void
  getSessionId: () => string
  getServerId: () => string
  setActivePaneId: (id: string | undefined) => void
  setStatus: (id: string, status: SessionStatus) => void
  deleteFn: (id: string) => void
  onChanged?: () => void
}

export function splitPane(ctx: PaneContext, paneId: string, direction: 'horizontal' | 'vertical') {
  const pendingId = `pending-split-${Date.now()}`
  const newPane: PaneNode = {
    id: pendingId,
    type: 'terminal',
    sessionId: pendingId,
    serverId: ctx.getServerId()
  }

  const currentTree = ctx.getPaneTree()
  let paneTree: PaneNode
  if (!currentTree || currentTree.type === 'terminal') {
    const existingPane: PaneNode = currentTree || {
      id: ctx.getSessionId(),
      type: 'terminal',
      sessionId: ctx.getSessionId(),
      serverId: ctx.getServerId()
    }
    paneTree = {
      id: `split-${Date.now()}`,
      type: 'split',
      direction,
      children: [existingPane, newPane],
      ratio: 0.5
    }
  } else {
    paneTree = JSON.parse(JSON.stringify(currentTree)) as PaneNode
    if (!replacePaneWithSplit(paneTree, paneId, direction, newPane)) {
      paneTree = {
        id: `split-${Date.now()}`,
        type: 'split',
        direction,
        children: [paneTree, newPane],
        ratio: 0.5
      }
    }
  }

  ctx.setPaneTree(paneTree)
  ctx.setActivePaneId(pendingId)
  ctx.setStatus(pendingId, 'connecting')
  ctx.onChanged?.()
}

export function closePane(ctx: PaneContext, paneId: string) {
  const tree = ctx.getPaneTree()
  if (!tree) return
  const result = removePaneFromTree(tree, paneId)
  if (result === null) {
    ctx.setPaneTree(undefined)
    ctx.setActivePaneId(undefined)
  } else {
    ctx.setPaneTree(result)
    ctx.setActivePaneId(findFirstTerminalId(result))
  }
  ctx.deleteFn(paneId)
  ctx.onChanged?.()
}

export function handleRatioChange(ctx: PaneContext, splitId: string, ratio: number) {
  const tree = ctx.getPaneTree()
  if (!tree) return
  updateRatio(tree, splitId, ratio)
  ctx.onChanged?.()
}
