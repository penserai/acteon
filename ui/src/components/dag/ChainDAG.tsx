import { useMemo, useState, useCallback } from 'react'
import {
  ReactFlow,
  Background,
  Controls,
  type Node,
  type Edge,
  Position,
  MarkerType,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import type { ChainDetailResponse, ChainStepConfig, BranchCondition, DagResponse, DagNode } from '../../types'
import { StepNode } from './StepNode'
import { SubChainNode } from './SubChainNode'
import styles from './ChainDAG.module.css'

interface ChainDAGProps {
  chain: ChainDetailResponse
  stepConfigs?: ChainStepConfig[]
  dag?: DagResponse
  onSelectStep?: (stepName: string) => void
  onNavigateChain?: (chainId: string) => void
}

const nodeTypes = { step: StepNode, sub_chain: SubChainNode }

export function ChainDAG({ chain, stepConfigs, dag, onSelectStep, onNavigateChain }: ChainDAGProps) {
  const [expandedSubChains, setExpandedSubChains] = useState<Set<string>>(new Set())

  const toggleExpand = useCallback((name: string) => {
    setExpandedSubChains((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }, [])

  const handleNavigateChild = useCallback((chainId: string) => {
    onNavigateChain?.(chainId)
  }, [onNavigateChain])

  const { nodes, edges } = useMemo(() => {
    // If a server-provided DAG is available, build from that
    if (dag) {
      return buildFromDag(dag, expandedSubChains, toggleExpand, handleNavigateChild)
    }

    // Otherwise fall back to the existing client-side logic
    return buildFromChain(chain, stepConfigs)
  }, [chain, stepConfigs, dag, expandedSubChains, toggleExpand, handleNavigateChild])

  const onNodeClick = useCallback((_: unknown, node: Node) => {
    onSelectStep?.(node.id)
  }, [onSelectStep])

  return (
    <div className={styles.container}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodeClick={onNodeClick}
        fitView
        fitViewOptions={{ padding: 0.3 }}
        proOptions={{ hideAttribution: true }}
        nodesDraggable={false}
      >
        <Background gap={20} size={1} color="var(--color-gray-200)" />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  )
}

/** Build nodes/edges from the server-provided DagResponse. */
function buildFromDag(
  dag: DagResponse,
  expandedSubChains: Set<string>,
  onToggleExpand: (name: string) => void,
  onNavigateChild: (chainId: string) => void,
  prefix = '',
  offsetX = 0,
  offsetY = 0,
): { nodes: Node[]; edges: Edge[] } {
  const nodeList: Node[] = []
  const edgeList: Edge[] = []
  const executionSet = new Set(dag.execution_path)

  // Layout the DAG nodes
  const positions = layoutDagNodes(dag.nodes, dag.edges)

  dag.nodes.forEach((dagNode) => {
    const nodeId = prefix ? `${prefix}::${dagNode.name}` : dagNode.name
    const isExecuted = executionSet.has(dagNode.name)
    const isActive = dag.status === 'running' && dagNode.status === 'pending' && isExecuted
    const pos = positions[dagNode.name] ?? { x: 0, y: 0 }

    if (dagNode.node_type === 'sub_chain') {
      const isExpanded = expandedSubChains.has(nodeId)
      const childStepCount = dagNode.children?.nodes.length

      nodeList.push({
        id: nodeId,
        type: 'sub_chain',
        position: { x: pos.x + offsetX, y: pos.y + offsetY },
        data: {
          label: dagNode.name,
          status: dagNode.status ?? 'pending',
          isActive,
          isExecuted,
          subChainName: dagNode.sub_chain_name ?? dagNode.name,
          childChainId: dagNode.child_chain_id,
          childStepCount,
          expanded: isExpanded,
          onToggleExpand,
          onNavigateChild,
        },
        sourcePosition: Position.Bottom,
        targetPosition: Position.Top,
      })

      // When expanded, flatten child nodes into the graph with offset
      if (isExpanded && dagNode.children) {
        const childPrefix = nodeId
        const childResult = buildFromDag(
          dagNode.children,
          expandedSubChains,
          onToggleExpand,
          onNavigateChild,
          childPrefix,
          pos.x + offsetX + 40,
          pos.y + offsetY + 80,
        )
        nodeList.push(...childResult.nodes)
        edgeList.push(...childResult.edges)
      }
    } else {
      nodeList.push({
        id: nodeId,
        type: 'step',
        position: { x: pos.x + offsetX, y: pos.y + offsetY },
        data: {
          label: dagNode.name,
          status: dagNode.status ?? 'pending',
          isActive,
          isExecuted,
          error: undefined,
        },
        sourcePosition: Position.Bottom,
        targetPosition: Position.Top,
      })
    }
  })

  // Build edges from DagResponse edges
  dag.edges.forEach((dagEdge, idx) => {
    const sourceId = prefix ? `${prefix}::${dagEdge.source}` : dagEdge.source
    const targetId = prefix ? `${prefix}::${dagEdge.target}` : dagEdge.target
    const isOnPath = dagEdge.on_execution_path

    edgeList.push({
      id: `${sourceId}-${targetId}-${idx}`,
      source: sourceId,
      target: targetId,
      label: dagEdge.label,
      animated: isOnPath,
      style: {
        stroke: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)',
        strokeWidth: isOnPath ? 3 : 1,
      },
      markerEnd: {
        type: MarkerType.ArrowClosed,
        color: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)',
      },
      labelStyle: { fontSize: 10, fill: 'var(--color-gray-500)' },
      labelBgStyle: { fill: 'var(--color-gray-0)', fillOpacity: 0.8 },
    })
  })

  return { nodes: nodeList, edges: edgeList }
}

/** Layout DAG nodes using BFS layering. */
function layoutDagNodes(
  dagNodes: DagNode[],
  dagEdges: { source: string; target: string }[],
): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {}
  if (dagNodes.length === 0) return positions

  // Build adjacency
  const children = new Map<string, string[]>()
  dagNodes.forEach((n) => children.set(n.name, []))
  dagEdges.forEach((e) => {
    const list = children.get(e.source)
    if (list) list.push(e.target)
  })

  // BFS layering from first node
  const layers: string[][] = []
  const visited = new Set<string>()
  let currentLayer = [dagNodes[0].name]
  visited.add(dagNodes[0].name)

  while (currentLayer.length > 0) {
    layers.push(currentLayer)
    const nextLayer: string[] = []
    for (const name of currentLayer) {
      for (const child of children.get(name) ?? []) {
        if (!visited.has(child)) {
          visited.add(child)
          nextLayer.push(child)
        }
      }
    }
    currentLayer = nextLayer
  }

  // Add unvisited nodes
  for (const n of dagNodes) {
    if (!visited.has(n.name)) {
      layers.push([n.name])
    }
  }

  // Position nodes
  layers.forEach((layer, layerIdx) => {
    const totalWidth = layer.length * 200
    const startX = (600 - totalWidth) / 2 + 100
    layer.forEach((name, nodeIdx) => {
      positions[name] = { x: startX + nodeIdx * 200, y: layerIdx * 120 }
    })
  })

  return positions
}

/** Build nodes/edges from the existing chain detail + step configs (original logic). */
function buildFromChain(
  chain: ChainDetailResponse,
  stepConfigs?: ChainStepConfig[],
): { nodes: Node[]; edges: Edge[] } {
  const executionSet = new Set(chain.execution_path)
  const stepStatusMap = new Map(chain.steps.map((s) => [s.name, s]))

  const nodeList: Node[] = []
  const edgeList: Edge[] = []

  const stepNames = stepConfigs?.map((s) => s.name) ?? chain.steps.map((s) => s.name)
  const positions = layoutSteps(stepNames, stepConfigs)

  stepNames.forEach((name) => {
    const status = stepStatusMap.get(name)
    const config = stepConfigs?.find((c) => c.name === name)
    const isExecuted = executionSet.has(name)
    const isActive = chain.status === 'running' && status?.status === 'pending' &&
      chain.execution_path.length > 0 &&
      chain.steps[chain.current_step]?.name === name

    const isSubChain = !!config?.sub_chain || !!status?.sub_chain

    nodeList.push({
      id: name,
      type: isSubChain ? 'sub_chain' : 'step',
      position: positions[name] ?? { x: 0, y: 0 },
      data: isSubChain ? {
        label: name,
        status: status?.status ?? 'pending',
        isActive,
        isExecuted,
        subChainName: config?.sub_chain ?? status?.sub_chain ?? name,
        childChainId: status?.child_chain_id,
        expanded: false,
        onToggleExpand: () => {},
        onNavigateChild: () => {},
      } : {
        label: name,
        status: status?.status ?? 'pending',
        isActive,
        isExecuted,
        error: status?.error,
      },
      sourcePosition: Position.Bottom,
      targetPosition: Position.Top,
    })
  })

  // Build edges
  if (stepConfigs) {
    stepConfigs.forEach((config) => {
      config.branches?.forEach((branch: BranchCondition, bi: number) => {
        const isOnPath = executionSet.has(config.name) && executionSet.has(branch.target)
        edgeList.push({
          id: `${config.name}-${branch.target}-b${bi}`,
          source: config.name,
          target: branch.target,
          label: `${branch.field} ${branch.operator} ${branch.value ?? ''}`,
          animated: isOnPath,
          style: {
            stroke: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)',
            strokeWidth: isOnPath ? 3 : 1,
          },
          markerEnd: { type: MarkerType.ArrowClosed, color: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)' },
          labelStyle: { fontSize: 10, fill: 'var(--color-gray-500)' },
          labelBgStyle: { fill: 'var(--color-gray-0)', fillOpacity: 0.8 },
        })
      })

      if (config.default_next) {
        const isOnPath = executionSet.has(config.name) && executionSet.has(config.default_next)
        edgeList.push({
          id: `${config.name}-${config.default_next}-default`,
          source: config.name,
          target: config.default_next,
          label: config.branches?.length ? 'default' : undefined,
          animated: isOnPath,
          style: {
            stroke: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)',
            strokeWidth: isOnPath ? 3 : 1,
          },
          markerEnd: { type: MarkerType.ArrowClosed, color: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)' },
          labelStyle: { fontSize: 10, fill: 'var(--color-gray-400)' },
        })
      }
    })
  } else {
    for (let i = 0; i < stepNames.length - 1; i++) {
      const isOnPath = executionSet.has(stepNames[i]) && executionSet.has(stepNames[i + 1])
      edgeList.push({
        id: `${stepNames[i]}-${stepNames[i + 1]}`,
        source: stepNames[i],
        target: stepNames[i + 1],
        animated: isOnPath,
        style: {
          stroke: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)',
          strokeWidth: isOnPath ? 3 : 1,
        },
        markerEnd: { type: MarkerType.ArrowClosed, color: isOnPath ? 'var(--color-primary-400)' : 'var(--color-gray-300)' },
      })
    }
  }

  return { nodes: nodeList, edges: edgeList }
}

function layoutSteps(stepNames: string[], stepConfigs?: ChainStepConfig[]): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {}

  if (!stepConfigs || stepConfigs.length === 0) {
    stepNames.forEach((name, i) => {
      positions[name] = { x: 250, y: i * 120 }
    })
    return positions
  }

  const children = new Map<string, string[]>()
  stepConfigs.forEach((config) => {
    const targets = new Set<string>()
    config.branches?.forEach((b) => targets.add(b.target))
    if (config.default_next) targets.add(config.default_next)
    children.set(config.name, [...targets])
  })

  const layers: string[][] = []
  const visited = new Set<string>()
  let currentLayer = [stepConfigs[0].name]
  visited.add(stepConfigs[0].name)

  while (currentLayer.length > 0) {
    layers.push(currentLayer)
    const nextLayer: string[] = []
    for (const name of currentLayer) {
      for (const child of children.get(name) ?? []) {
        if (!visited.has(child)) {
          visited.add(child)
          nextLayer.push(child)
        }
      }
    }
    currentLayer = nextLayer
  }

  for (const name of stepNames) {
    if (!visited.has(name)) {
      layers.push([name])
    }
  }

  layers.forEach((layer, layerIdx) => {
    const totalWidth = layer.length * 200
    const startX = (600 - totalWidth) / 2 + 100
    layer.forEach((name, nodeIdx) => {
      positions[name] = { x: startX + nodeIdx * 200, y: layerIdx * 120 }
    })
  })

  return positions
}
