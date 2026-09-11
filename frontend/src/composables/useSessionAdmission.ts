import type { useApi } from '@/composables/useApi'

/**
 * Body the server answers with (HTTP 202) when a guard wants a second person
 * to approve the request before it can proceed.
 */
export interface AwaitApprovalBody {
  decision: 'await_approval'
  approvalRequired: true
  requestId: string
  error?: string
}

export function isAwaitApproval(v: unknown): v is AwaitApprovalBody {
  return !!v && typeof v === 'object' && (v as AwaitApprovalBody).approvalRequired === true
}

const POLL_MS = 4000

/**
 * POST `path` and, when the server asks for approval first, wait for the
 * decision and retry once it is approved. Rejection, expiry or withdrawal
 * become an Error so callers can show it like any other connection failure.
 */
export async function postWithAdmission<T>(
  api: ReturnType<typeof useApi>,
  path: string,
  body: unknown,
  onWaiting?: (requestId: string, message: string) => void,
  isCancelled?: () => boolean
): Promise<T> {
  for (let attempt = 0; attempt < 2; attempt++) {
    const res = await api.post<T | AwaitApprovalBody>(path, body)
    if (!isAwaitApproval(res)) return res as T
    if (attempt === 1) throw new Error('审批已通过但请求仍被拦截，请重试')
    onWaiting?.(res.requestId, res.error || '需要审批人批准后才能继续')
    await waitForDecision(api, res.requestId, isCancelled)
  }
  throw new Error('unreachable')
}

async function waitForDecision(api: ReturnType<typeof useApi>, requestId: string, isCancelled?: () => boolean) {
  for (;;) {
    await new Promise((r) => setTimeout(r, POLL_MS))
    if (isCancelled?.()) throw new Error('已取消等待审批')
    let status: string
    let reason = ''
    try {
      const req = await api.get<{ status: string; decisionComment?: string | null; cancelReason?: string | null }>(
        `/approvals/${requestId}`
      )
      status = req.status
      reason = req.decisionComment || req.cancelReason || ''
    } catch (e) {
      if ((e as Error).message === 'Unauthorized') throw e
      continue
    }
    if (status === 'pending') continue
    if (status === 'approved') return
    const label = status === 'rejected' ? '审批被拒绝' : status === 'expired' ? '审批已过期' : '审批申请已撤回'
    throw new Error(reason ? `${label}: ${reason}` : label)
  }
}
