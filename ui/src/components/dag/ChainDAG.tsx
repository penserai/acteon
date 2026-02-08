import { useMemo, useCallback } from 'react'
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
import type { ChainDetailResponse, ChainStepConfig, BranchCondition } from '../../types'
import { StepNode } from './StepNode'
import styles from './ChainDAG.module.css'

interface ChainDAGProps {
  chain: ChainDetailResponse
  stepConfigs?: ChainStepConfig[]
  onSelectStep?: (stepName: string) => void
}

const nodeTypes = { step: StepNode }

export function ChainDAG({ chain, stepConfigs, onSelectStep }: ChainDAGProps) {
  const { nodes, edges } = useMemo(() => {
    const executionSet = new Set(chain.execution_path)
    const stepStatusMap = new Map(chain.steps.map((s) => [s.name, s]))

    const nodeList: Node[] = []
    const edgeList: Edge[] = []

    // Build adjacency from step configs or linear from steps
    const stepNames = stepConfigs?.map((s) => s.name) ?? chain.steps.map((s) => s.name)

    // Layout: use Dagre-like simple layout (top-to-bottom)
    const positions = layoutSteps(stepNames, stepConfigs)

    stepNames.forEach((name) => {
      const status = stepStatusMap.get(name)
      const isExecuted = executionSet.has(name)
      const isActive = chain.status === 'running' && status?.status === 'pending' &&
        chain.execution_path.length > 0 &&
        chain.steps[chain.current_step]?.name === name

      nodeList.push({
        id: name,
        type: 'step',
        position: positions[name] ?? { x: 0, y: 0 },
        data: {
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
        // Branch edges
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

        // Default next edge
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
      // Linear chain - connect step i to i+1
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
  }, [chain, stepConfigs])

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

function layoutSteps(stepNames: string[], stepConfigs?: ChainStepConfig[]): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {}

  if (!stepConfigs || stepConfigs.length === 0) {
    // Simple linear layout
    stepNames.forEach((name, i) => {
      positions[name] = { x: 250, y: i * 120 }
    })
    return positions
  }

  // Build adjacency for simple layered layout
  const children = new Map<string, string[]>()
  stepConfigs.forEach((config) => {
    const targets = new Set<string>()
    config.branches?.forEach((b) => targets.add(b.target))
    if (config.default_next) targets.add(config.default_next)
    children.set(config.name, [...targets])
  })

  // BFS layering
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

  // Add unvisited nodes
  for (const name of stepNames) {
    if (!visited.has(name)) {
      layers.push([name])
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
