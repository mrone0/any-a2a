const $=id=>document.getElementById(id)
let source='url',agents=[]
function showStatus(message, ok=false){const node=$('status');node.textContent=message;node.className=`status show ${ok?'ok':''}`}
const invoke=(cmd,args)=>window.__TAURI__.core.invoke(cmd,args)
const controlWindow = action => invoke('window_control', {action}).catch(error => console.error('Window action failed', error));
document.querySelectorAll('[data-window]').forEach(button => button.onclick = () => controlWindow(button.dataset.window));
$('window-bar').onmousedown = event => {
  if (event.button === 0 && !event.target.closest('button') && event.detail !== 2) controlWindow('drag');
};
$('window-bar').ondblclick = event => { if (!event.target.closest('button')) controlWindow('maximize'); };
async function api(body=null){return invoke('cards_api',{body})}
function view(name){document.querySelectorAll('.view').forEach(x=>x.classList.toggle('active',x.id===name));document.querySelectorAll('.nav').forEach(x=>x.classList.toggle('active',x.dataset.view===name))}
async function load(){try{agents=await api();if(!Array.isArray(agents))throw Error('服务返回异常');render();$('dot').classList.add('ok');$('service-text').textContent='运行中';$('setting-service').textContent='运行中'}catch(e){$('dot').classList.remove('ok');$('service-text').textContent='服务不可用';$('setting-service').textContent='不可用';showStatus(`服务错误：${e.message}`)}}
function render() {
  $('stats').textContent = `${agents.length} 个 Agent · 选择一个开始测试`;
  const list = $('list');
  const select = $('run-agent');
  list.replaceChildren();
  select.replaceChildren();
  if (!agents.length) {
    const empty = document.createElement('div');
    empty.className = 'empty';
    empty.textContent = '还没有 Agent。添加你的第一个连接。';
    list.append(empty);
  }
  for (const agent of agents) {
    const row = document.createElement('article');
    row.className = 'agent';
    // Only static markup: card names, IDs and descriptions are untrusted data.
    row.innerHTML = `<div class="agent-avatar" aria-hidden="true"></div>
      <div class="agent-content"><div class="agent-heading"><strong></strong><span class="source-badge"></span></div>
      <p class="agent-description"></p>
      <details class="agent-details"><summary>查看详情</summary><p></p><dl><dt>协议</dt><dd></dd><dt>服务地址</dt><dd class="endpoint"></dd></dl></details></div>
      <div class="agent-actions"><button class="agent-run" type="button">运行 <span aria-hidden="true">↗</span></button><button class="agent-delete" type="button">删除</button></div>`;
    row.querySelector('.agent-avatar').textContent = Array.from(agent.info.name || 'A')[0].toUpperCase();
    row.querySelector('strong').textContent = agent.info.name;
    row.querySelector('.source-badge').textContent = agent.source === 'url' ? 'URL' : '手动';
    row.querySelector('.agent-description').textContent = agent.info.description;
    row.querySelector('.agent-details p').textContent = agent.info.description;
    row.querySelector('dd').textContent = `${agent.info.binding || 'JSONRPC'} · ${agent.info.version || '—'}`;
    row.querySelector('.endpoint').textContent = agent.info.endpoint || '—';
    row.querySelector('button').onclick = () => { view('run'); select.value = agent.id; updateRequestForm(); };
    row.querySelector('.agent-delete').onclick = () => {
      if (row.querySelector('.delete-confirm')) return;
      const panel = document.createElement('div'); panel.className = 'delete-confirm';
      const text = document.createElement('p'); text.textContent = `删除「${agent.info.name}」？仅移除本地记录，不取消远端任务；历史配置和凭据不会被擦除。`;
      const confirm = document.createElement('button'); confirm.className = 'agent-delete'; confirm.textContent = '确认删除';
      const cancel = document.createElement('button'); cancel.className = 'secondary'; cancel.textContent = '取消';
      cancel.onclick = () => panel.remove();
      confirm.onclick = async () => {
        confirm.disabled = true;
        try {
          const result = await invoke('delete_card', { id: agent.id });
          if (result.error || result.deleted !== true) throw Error(result.error || '删除未完成');
          await load();
        } catch (error) { text.textContent = `删除失败：${error.message || error}`; confirm.disabled = false; }
      };
      panel.append(text, cancel, confirm); row.append(panel);
    };
    list.append(row);
    const option = document.createElement('option');
    option.value = agent.id;
    option.textContent = agent.info.name;
    select.append(option);
  }
}
document.querySelectorAll('[data-view]').forEach(x=>x.onclick=()=>view(x.dataset.view));document.querySelectorAll('[data-goto]').forEach(x=>x.onclick=()=>view(x.dataset.goto));document.querySelectorAll('[data-source]').forEach(x=>x.onclick=()=>{source=x.dataset.source;document.querySelectorAll('.tab').forEach(t=>t.classList.toggle('active',t.dataset.source===source));$('url-field').hidden=source!=='url';$('manual-field').hidden=source!=='manual'})
$('form').onsubmit=async e=>{e.preventDefault();$('save').disabled=true;showStatus('正在验证并保存…');try{const body={auth:{bearerToken:$('auth-token').value,headers:$('auth-headers').value.trim()?JSON.parse($('auth-headers').value):{}}};if(source==='url')body.cardUrl=$('url').value.trim();else body.agentCard=JSON.parse($('card').value);const r=await api(body);if(r.error)throw Error(r.error);if(!r||typeof r!=='object')throw Error('保存返回异常');showStatus('已保存',true);$('auth-token').value='';$('auth-headers').value='';await load();view('agents')}catch(e){showStatus(`保存失败：${e.message}`)}finally{$('save').disabled=false}}
let activeRunId = null;
$('cancel-run').onclick = async () => {
  const runId = activeRunId;
  if (!runId) return;
  $('cancel-run').disabled = true;
  try {
    const response = await invoke('cancel_run', {runId});
    if (activeRunId !== runId) return;
    $('cancel-status').textContent = response.error || response.message;
    if (response.error) $('cancel-run').disabled = false;
  } catch (error) {
    if (activeRunId !== runId) return;
    $('cancel-status').textContent = `取消未确认：${error.message || error}`;
    $('cancel-run').disabled = false;
  }
};
$('run-button').onclick = async () => {
  const id = $('run-agent').value;
  if (!id) return;
  if (!$('message').value.trim()) { $('result').textContent = '请输入测试消息'; $('message').focus(); return; }
  $('run-button').disabled = true;
  activeRunId = crypto.randomUUID();
  $('cancel-run').disabled = false;
  $('cancel-status').textContent = '可请求取消；若尚未取得任务 ID，将等待当前请求返回。';
  $('result').textContent = '正在等待 Agent 回复…';
  $('result').classList.remove('response-error');
  $('response-meta').textContent = '运行中';
  $('response-json').textContent = ''; $('response-details').open = false;
  const started = performance.now();
  try {
    const response = await invoke('run_card', {id, runId: activeRunId, message: $('message').value, configuration: requestConfiguration()});
    $('response-json').textContent = JSON.stringify(response, null, 2);
    $('cancel-status').textContent = response.state === 'canceled' ? '远端已确认取消。' : response.error ? '运行结束，但未确认取消；远端任务可能仍在运行。' : '任务已返回，无需取消。';
    $('result').textContent = response.error || (response.state === 'canceled' ? '任务已取消（远端确认）' : response.text || '任务已返回，但没有文本回复。');
    $('result').classList.toggle('response-error', Boolean(response.error));
    $('response-meta').textContent = `${response.error ? '失败' : response.state === 'canceled' ? '已取消' : '已返回'} · ${((performance.now() - started) / 1000).toFixed(1)} 秒`;
  } catch (error) {
    $('result').textContent = `请求失败：${error.message || error}`;
    $('cancel-status').textContent = '本地请求中断，远端任务状态未知；未确认取消。';
    $('result').classList.add('response-error'); $('response-meta').textContent = '失败';
  } finally {
    activeRunId = null; $('cancel-run').disabled = true;
    $('result').scrollTop = 0; $('response-json').scrollTop = 0; $('run-button').disabled = false;
  }
};
async function scanClients() {
  $('scan-clients').disabled = true;
  $('scan-status').textContent = '正在扫描本机安装位置…';
  try {
    const clients = await invoke('scan_clients');
    $('client-list').replaceChildren();
    for (const client of clients) {
      const row = document.createElement('article');
      row.className = 'client-row';
      const heading = document.createElement('strong'); heading.textContent = client.name;
      const status = document.createElement('span'); status.className = client.detected ? 'client-found' : 'client-missing';
      status.textContent = client.detected ? '已检测到' : '未检测到';
      const path = document.createElement('p'); path.textContent = client.executable || '常见路径中未找到 CLI';
      const integration = document.createElement('small'); integration.textContent = 'any-a2a 接入状态：未验证';
      row.append(heading, status, path, integration); $('client-list').append(row);
    }
    $('scan-status').textContent = `扫描完成 · 检测到 ${clients.filter(c => c.detected).length} / ${clients.length} 个客户端`;
  } catch (error) { $('scan-status').textContent = `扫描失败：${error.message || error}`; }
  finally { $('scan-clients').disabled = false; }
}
function renderClientEntrances() {
  $('client-list').replaceChildren();
  for (const [id, name] of [['claude', 'Claude Code'], ['codex', 'Codex'], ['pi', 'pi'], ['dsh', 'DeepSeek Harness'], ['opencode', 'OpenCode'], ['hermes', 'Hermes']]) {
    const row = document.createElement('article'); row.className = 'client-row';
    const heading = document.createElement('strong'); heading.textContent = name;
    const detail = document.createElement('p');
    detail.textContent = id === 'dsh' ? '支持 CLI 或 pnpm 源码启动；使用实际 profile 的 patch 路径配置，不要求全局 dsh 命令。' : id === 'pi' ? '已支持扩展式远端子 Agent 委派、后台运行、进度查询和本地停止；无需保持 App 运行。' : '后续版本支持';
    const action = document.createElement('button'); action.className = 'secondary';
    action.textContent = id === 'dsh' ? '配置 DSH' : id === 'pi' ? '连接 Pi' : '暂未开放'; action.disabled = !['dsh','pi'].includes(id);
    action.onclick = async () => {
      if (id === 'pi') {
        $('pi-config').hidden = false; action.disabled = true;
        $('pi-command').textContent = '正在解析本机路径…';
        try {
          const setup = await invoke('pi_setup'); $('pi-command').textContent = setup.command;
          const platform = {windows:'Windows', macos:'macOS', linux:'Linux'}[setup.platform] || setup.platform;
          $('pi-setup-hint').textContent = `当前系统：${platform}。在 ${setup.shell} 终端中执行以下命令，启动已加载 A2A 扩展的 Pi。`;
        }
        catch (error) { $('pi-setup-hint').textContent = '暂时无法生成启动命令。'; $('pi-command').textContent = `配置准备失败：${error.message || error}`; }
        finally { action.disabled = false; }
        return;
      }
      $('dsh-config').hidden = false;
      $('install-agent-count').textContent = `将应用全部 ${agents.length} 个已保存 Agent，无需逐个选择。`;
      action.disabled = true; $('dsh-discovery-status').textContent = '正在检测 DSH profile…';
      try {
        const paths = await invoke('discover_dsh');
        $('dsh-candidates').replaceChildren();
        for (const path of paths) { const option = document.createElement('option'); option.value = path; option.textContent = path; $('dsh-candidates').append(option); }
        $('dsh-candidates').disabled = !paths.length;
        $('dsh-patch').value = paths[0] || '';
        $('dsh-discovery-status').textContent = paths.length ? `发现 ${paths.length} 个配置${paths.length > 1 ? '，请选择实际使用的 profile' : '，已自动填入'}。尚未修改配置。` : '常见位置中未发现配置，请手动指定自定义 profile 路径。';
      } catch (error) { $('dsh-discovery-status').textContent = `检测失败：${error.message || error}`; }
      finally { action.disabled = false; }
    };
    row.append(heading, detail, action); $('client-list').append(row);
  }
  $('scan-status').textContent = '选择安装目标后再检查环境；启动时不扫描本机。';
}
function updateRequestForm() {
  const agent = agents.find(a => a.id === $('run-agent').value);
  const version = agent?.info.version;
  $('request-version').textContent = version ? `A2A ${version}` : '未选择 Agent';
  $('mode-label').textContent = version === '1.0' ? 'returnImmediately · 立即返回任务' : 'blocking · 等待任务结果';
  $('request-mode').value = 'default'; $('history-length').value = ''; $('output-modes').value = '';
}
function requestConfiguration() {
  const version = agents.find(a => a.id === $('run-agent').value)?.info.version;
  const config = {};
  if ($('request-mode').value !== 'default') config[version === '1.0' ? 'returnImmediately' : 'blocking'] = $('request-mode').value === 'true';
  if ($('history-length').value !== '') config.historyLength = Number($('history-length').value);
  if ($('output-modes').value.trim()) config.acceptedOutputModes = $('output-modes').value.split(',').map(s => s.trim()).filter(Boolean);
  return config;
}
$('dsh-candidates').onchange = () => { $('dsh-patch').value = $('dsh-candidates').value; };
$('dsh-apply').onclick = () => {
  if (!agents.length) { $('dsh-apply-status').textContent = '请先添加 Agent'; return; }
  if (!$('dsh-patch').value.trim()) { $('dsh-apply-status').textContent = '请先选择 DSH 配置文件'; return; }
  $('dsh-apply-confirm').hidden = false;
};
$('dsh-apply-cancel').onclick = () => { $('dsh-apply-confirm').hidden = true; };
$('dsh-apply-commit').onclick = async () => {
  $('dsh-apply-commit').disabled = true;
  $('dsh-apply-status').textContent = '正在备份并写入配置…';
  try {
    const result = await invoke('apply_dsh', { path: $('dsh-patch').value.trim() });
    $('dsh-apply-status').textContent = `配置已写入（运行接入未验证）。备份：${result.backup}\n${result.message}`;
    $('dsh-apply-confirm').hidden = true;
  } catch (error) { $('dsh-apply-status').textContent = `应用失败：${error.message || error}`; }
  finally { $('dsh-apply-commit').disabled = false; }
};
$('run-agent').onchange = updateRequestForm;
let configOriginal = '';
$('config-read').onclick = async () => {
  try { configOriginal = await invoke('read_config'); $('config-text').value = configOriginal; $('config-editor').hidden = false; }
  catch(e) { $('config-status').textContent = String(e); }
};
$('config-close').onclick = () => { $('config-editor').hidden = true; $('config-text').value = ''; configOriginal = ''; };
$('config-open').onclick = () => invoke('open_config').catch(e => $('config-status').textContent = String(e));
$('config-save').onclick = async () => {
  $('config-save').disabled = true;
  try {
    const result = await invoke('save_config', {content:$('config-text').value, original:configOriginal});
    configOriginal = await invoke('read_config'); $('config-text').value = configOriginal;
    $('config-status').textContent = `已保存，备份：${result.backup}`; await load();
  } catch(e) { $('config-status').textContent = `保存失败：${e}`; }
  finally { $('config-save').disabled = false; }
};
invoke('config_location').then(path => $('config-path').textContent = path).catch(e => $('config-status').textContent = String(e));
renderClientEntrances();
load().then(updateRequestForm);
