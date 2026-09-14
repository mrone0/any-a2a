/** Shared any-a2a local-service transport. Host adapters only translate their native request into this contract. */
export async function runService({ serviceUrl, serviceToken, agentId, message, signal }) {
  if (typeof serviceUrl !== 'string' || !/^https?:\/\//.test(serviceUrl)) throw new Error('any-a2a: serviceUrl must be HTTP(S)')
  if (typeof agentId !== 'string' || !agentId.trim()) throw new Error('any-a2a: agentId is required')
  const headers = { 'content-type': 'application/json' }
  if (serviceToken) headers.authorization = `Bearer ${serviceToken}`
  const response = await fetch(`${serviceUrl.replace(/\/$/, '')}/api/run`, {
    method: 'POST', headers, body: JSON.stringify({ id: agentId, message }), signal,
  })
  const value = await response.json().catch(() => null)
  if (!response.ok) throw new Error(value?.error || `any-a2a service HTTP ${response.status}`)
  if (!value || typeof value !== 'object' || typeof value.text !== 'string' || typeof value.state !== 'string') throw new Error('any-a2a: invalid service response')
  return value
}
