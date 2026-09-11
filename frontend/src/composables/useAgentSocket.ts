import { ref } from 'vue'
import { useAuthStore } from '@/stores/auth'

export interface AgentMessage {
  role: 'user' | 'assistant' | 'system'
  content: string
  toolUse?: ToolUseBlock[]
  timestamp: number
}

export interface ToolUseBlock {
  id: string
  name: string
  input: string
  result?: string
}

export interface PermissionRequest {
  toolName: string
  input: Record<string, unknown>
  requestId: string
  description: string
  risk?: RiskInfo
  target?: string
  /** Set when policy routes this call to a second person instead of the requester. */
  secondPerson?: SecondPersonApproval
}

export interface SecondPersonApproval {
  approvalRequestId: string
  expiresAt: string
  summary: string
  riskLevel?: string
  policyVersion?: string
}

export interface RiskInfo {
  level: 'low' | 'medium' | 'high' | 'critical'
  reason: string
}

export interface UserQuestion {
  id: string
  question: string
  options: string[]
}

export type AgentStatus =
  'idle' | 'thinking' | 'tool_use' | 'waiting_approval' | 'waiting_input' | 'disconnected' | 'error'

export function useAgentSocket(agentId: string) {
  const auth = useAuthStore()
  const connected = ref(false)
  const status = ref<AgentStatus>('idle')
  const messages = ref<AgentMessage[]>([])
  const currentText = ref('')
  const currentToolUse = ref<ToolUseBlock[]>([])
  const tokenUsage = ref<{ input: number; output: number }>({ input: 0, output: 0 })
  const costUsd = ref(0)
  const pendingApproval = ref<PermissionRequest | null>(null)
  const pendingQuestion = ref<UserQuestion | null>(null)

  function pushSystem(content: string) {
    messages.value.push({ role: 'system', content, timestamp: Date.now() })
  }

  let ws: WebSocket | null = null
  let didOpen = false

  function connect() {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const url = `${protocol}//${location.host}/ws/agent/${agentId}?token=${auth.token}`
    didOpen = false
    ws = new WebSocket(url)

    ws.onopen = () => {
      connected.value = true
      didOpen = true
    }

    ws.onclose = () => {
      connected.value = false
      status.value = 'disconnected'
      if (!didOpen) {
        auth.check().then((valid) => {
          if (!valid) window.location.hash = '#/login'
        })
      }
    }

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        handleEvent(data)
      } catch {}
    }
  }

  function handleEvent(data: Record<string, unknown>) {
    const type = data.type as string

    if (type === 'system') {
      const subtype = data.subtype as string | undefined
      if (subtype === 'init') {
        messages.value.push({
          role: 'system',
          content: `Claude Code 已连接 (${(data as Record<string, unknown>).model || 'unknown'})`,
          timestamp: Date.now()
        })
      }
    } else if (type === 'assistant_delta') {
      status.value = 'thinking'
      const deltaText = (data as Record<string, unknown>).text as string
      if (deltaText) {
        const last = messages.value[messages.value.length - 1]
        if (last && last.role === 'assistant' && !last.toolUse) {
          last.content += deltaText
        } else {
          messages.value.push({
            role: 'assistant',
            content: deltaText,
            timestamp: Date.now()
          })
        }
      }
    } else if (type === 'assistant') {
      status.value = 'thinking'
      const msg = (data as Record<string, unknown>).message as Record<string, unknown> | undefined
      const contentBlocks = msg?.content as Array<Record<string, unknown>> | undefined
      if (contentBlocks) {
        let text = ''
        const tools: ToolUseBlock[] = []

        for (const block of contentBlocks) {
          if (block.type === 'text') {
            text += block.text as string
          } else if (block.type === 'tool_use') {
            status.value = 'tool_use'
            tools.push({
              id: block.id as string,
              name: block.name as string,
              input: typeof block.input === 'string' ? block.input : JSON.stringify(block.input, null, 2)
            })
          }
        }

        if (tools.length > 0) {
          // Tool use: always a new message
          messages.value.push({
            role: 'assistant',
            content: text,
            toolUse: tools,
            timestamp: Date.now()
          })
        } else if (text) {
          // Complete text: replace the last delta-accumulated message or create new
          const last = messages.value[messages.value.length - 1]
          if (last && last.role === 'assistant' && !last.toolUse) {
            last.content = text
          } else {
            messages.value.push({
              role: 'assistant',
              content: text,
              timestamp: Date.now()
            })
          }
        }
        currentText.value = ''
        currentToolUse.value = []
      }
    } else if (type === 'user') {
      // Tool result (auto-generated by claude)
      const msg = (data as Record<string, unknown>).message as Record<string, unknown> | undefined
      const contentBlocks = msg?.content as Array<Record<string, unknown>> | undefined
      if (contentBlocks) {
        for (const block of contentBlocks) {
          if (block.type === 'tool_result') {
            const toolId = block.tool_use_id as string
            const lastMsg = [...messages.value].reverse().find((m) => m.toolUse?.some((t) => t.id === toolId))
            if (lastMsg) {
              const tool = lastMsg.toolUse?.find((t) => t.id === toolId)
              if (tool) {
                const content = block.content as string | Array<Record<string, unknown>>
                tool.result = typeof content === 'string' ? content : JSON.stringify(content)
              }
            }
          }
        }
      }
    } else if (type === 'permission_request') {
      status.value = 'waiting_approval'
      const toolInfo = data.tool as Record<string, unknown> | undefined
      const riskData = data.risk as Record<string, unknown> | undefined
      pendingApproval.value = {
        toolName: (toolInfo?.name as string) || (data.tool as string) || 'unknown',
        input: (toolInfo?.input as Record<string, unknown>) || (data.input as Record<string, unknown>) || {},
        requestId: (data.id as string) || (data.request_id as string) || '',
        description: (data.description as string) || '',
        risk: riskData ? { level: riskData.level as RiskInfo['level'], reason: riskData.reason as string } : undefined,
        target: (data.target as string) || undefined
      }
    } else if (type === 'ask_user') {
      status.value = 'waiting_input'
      const opts = data.options
      pendingQuestion.value = {
        id: (data.id as string) || '',
        question: (data.question as string) || '',
        options: Array.isArray(opts) ? (opts as unknown[]).map((o) => String(o)) : []
      }
    } else if (type === 'permission_ack') {
      const ackStatus = data.status as string | undefined
      if (ackStatus === 'second_person_required') {
        pushSystem('此操作需要第二人审批，你的选择不会生效，请等待审批人处理')
      } else if (ackStatus && ackStatus !== 'applied') {
        pushSystem(`审批未生效 (${ackStatus})，请重试或等待新的审批请求`)
      }
    } else if (type === 'approval_pending') {
      // The gate took this call over: the requester waits, an approver decides.
      const callId = data.id as string
      const info: SecondPersonApproval = {
        approvalRequestId: (data.requestId as string) || '',
        expiresAt: (data.expiresAt as string) || '',
        summary: (data.summary as string) || '',
        riskLevel: (data.riskLevel as string) || undefined,
        policyVersion: (data.policyVersion as string) || undefined
      }
      if (pendingApproval.value && pendingApproval.value.requestId === callId) {
        pendingApproval.value = { ...pendingApproval.value, secondPerson: info }
      } else {
        pendingApproval.value = {
          toolName: 'unknown',
          input: {},
          requestId: callId,
          description: info.summary,
          secondPerson: info
        }
      }
      status.value = 'waiting_approval'
    } else if (type === 'approval_decided') {
      const approved = data.approved === true
      const outcome = data.status as string | undefined
      const by = data.decidedBy as string | undefined
      const comment = data.comment as string | undefined
      let text: string
      if (approved) text = `第二人审批已批准${by ? `（${by}）` : ''}`
      else if (outcome === 'rejected') text = `第二人审批已拒绝${by ? `（${by}）` : ''}`
      else if (outcome === 'expired') text = '审批申请已过期，操作被拒绝'
      else if (outcome === 'cancelled') text = '审批申请已撤回，操作被拒绝'
      else text = `审批未通过（${outcome || 'unknown'}），操作被拒绝`
      pushSystem(comment ? `${text}: ${comment}` : text)
      if (pendingApproval.value?.requestId === (data.id as string)) pendingApproval.value = null
      status.value = approved ? 'tool_use' : 'thinking'
    } else if (type === 'approval_failed') {
      pushSystem(`审批申请未能记录，操作被拒绝: ${(data.error as string) || ''}`)
      if (pendingApproval.value?.requestId === (data.id as string)) pendingApproval.value = null
      status.value = 'thinking'
    } else if (type === 'gap') {
      const dropped = (data.dropped as number | undefined) ?? 0
      pushSystem(`输出过快，已丢弃 ${dropped} 条事件；以上内容可能不完整`)
    } else if (type === 'result') {
      status.value = 'idle'
      pendingApproval.value = null
      pendingQuestion.value = null
      const usage = (data as Record<string, unknown>).usage as Record<string, number> | undefined
      if (usage) {
        tokenUsage.value = {
          input: usage.input_tokens || 0,
          output: usage.output_tokens || 0
        }
      }
      const cost =
        ((data as Record<string, unknown>).cost_usd as number | undefined) ??
        ((data as Record<string, unknown>).total_cost_usd as number | undefined)
      if (cost) {
        costUsd.value += cost
      }
      const stopReason = (data as Record<string, unknown>).stop_reason as string | undefined
      if (stopReason === 'max_turns') {
        pushSystem('已达到最大对话轮数限制，代理已停止')
      }
    } else if (type === 'user_message') {
      messages.value.push({
        role: 'user',
        content: (data as Record<string, unknown>).content as string,
        timestamp: Date.now()
      })
    } else if (type === 'replay_done') {
      status.value = 'idle'
    } else if (type === 'error') {
      status.value = 'error'
      messages.value.push({
        role: 'system',
        content: `错误: ${(data as Record<string, unknown>).error || 'unknown'}`,
        timestamp: Date.now()
      })
    } else if (type === 'exited') {
      status.value = 'disconnected'
      messages.value.push({
        role: 'system',
        content: '代理进程已退出',
        timestamp: Date.now()
      })
    }
  }

  function sendMessage(content: string) {
    if (!ws || ws.readyState !== WebSocket.OPEN) return
    if (pendingQuestion.value) {
      // The agent is waiting on ask_user: route the text as the answer.
      ws.send(JSON.stringify({ type: 'user_answer', id: pendingQuestion.value.id, answer: content }))
      pendingQuestion.value = null
    } else {
      ws.send(JSON.stringify({ type: 'message', content }))
    }
    messages.value.push({
      role: 'user',
      content,
      timestamp: Date.now()
    })
    status.value = 'thinking'
  }

  function answerQuestion(answer: string) {
    sendMessage(answer)
  }

  function respondPermission(granted: boolean) {
    if (!ws || ws.readyState !== WebSocket.OPEN || !pendingApproval.value) return
    // Flat protocol: the server forwards {type, id, approved} to the agent as-is.
    ws.send(
      JSON.stringify({
        type: 'permission_response',
        id: pendingApproval.value.requestId,
        approved: granted
      })
    )
    pendingApproval.value = null
    status.value = granted ? 'tool_use' : 'thinking'
  }

  function close() {
    ws?.close()
    ws = null
    connected.value = false
  }

  return {
    connected,
    status,
    messages,
    currentText,
    currentToolUse,
    tokenUsage,
    costUsd,
    pendingApproval,
    pendingQuestion,
    connect,
    sendMessage,
    answerQuestion,
    respondPermission,
    close
  }
}
