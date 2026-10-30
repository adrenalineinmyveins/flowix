import { useEffect, useState } from 'react';
import { ArrowLeft, Bot, Check, Globe, Search, Settings, UserPlus, UserMinus } from 'lucide-react';
import {
  api,
  AgentCard,
  MarketCard,
  RedactionStatus,
  SubscriptionEntry,
} from './api';

interface Props {
  onBack: () => void;
  onOpenSettings: () => void;
}

type Tab = 'mine' | 'market';

function cardTags(card: MarketCard): string[] {
  const raw = card.card?.specialtyTags;
  return Array.isArray(raw) ? raw.map(String).slice(0, 8) : [];
}

export function AgentHubScreen({ onBack, onOpenSettings }: Props) {
  const [tab, setTab] = useState<Tab>('mine');
  const [net, setNet] = useState<RedactionStatus | null>(null);

  useEffect(() => {
    api.redactionStatus().then(setNet).catch(() => undefined);
  }, [tab]);

  return (
    <div className="screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowLeft />
        </button>
        <div className="m-bar-title">Agent</div>
        <div />
      </header>

      <div className="hub-tabs">
        <button
          className={`hub-tab ${tab === 'mine' ? 'active' : ''}`}
          onClick={() => setTab('mine')}
        >
          <Bot size={15} />
          我的 Agent
        </button>
        <button
          className={`hub-tab ${tab === 'market' ? 'active' : ''}`}
          onClick={() => setTab('market')}
        >
          <Globe size={15} />
          市场
        </button>
      </div>

      {tab === 'mine' ? <MineTab net={net} onOpenSettings={onOpenSettings} /> : <MarketTab />}
    </div>
  );
}

function MineTab({
  net,
  onOpenSettings,
}: {
  net: RedactionStatus | null;
  onOpenSettings: () => void;
}) {
  const [card, setCard] = useState<AgentCard>({ name: '', bio: '', skills: [] });
  const [skillsText, setSkillsText] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');

  useEffect(() => {
    api
      .agentCardGet()
      .then((loaded) => {
        setCard(loaded);
        setSkillsText(loaded.skills.join('、'));
      })
      .catch(() => undefined);
  }, []);

  const doSave = async () => {
    if (busy) return;
    setBusy(true);
    setMessage('保存中…');
    try {
      const saved = await api.agentCardSet({
        ...card,
        skills: skillsText.split(/[,，、\s]+/).filter(Boolean),
      });
      setCard(saved);
      setSkillsText(saved.skills.join('、'));
      setMessage('名片已保存（仅存本机）');
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings-list">
      <div className="m-card">
        <div className="m-card-title">网络身份</div>
        {net?.registered ? (
          <>
            <div className="m-card-row">
              <span>身份地址</span>
              <span className="value" style={{ wordBreak: 'break-all' }}>{net.agentId}</span>
            </div>
            <div className="m-card-row">
              <span>中继</span>
              <span className="value" style={{ wordBreak: 'break-all' }}>{net.relayUrl}</span>
            </div>
            <div className="card-hint" style={{ textAlign: 'left' }}>
              其他用户的 agent 可向此地址发起咨询，收到的咨询进入「网络咨询」会话
            </div>
          </>
        ) : (
          <>
            <div className="card-hint" style={{ textAlign: 'left' }}>
              尚未注册网络身份。注册后本机 Agent 可被中继网络寻址与咨询。
            </div>
            <div style={{ padding: '12px 0 0' }}>
              <button className="m-btn-ghost" style={{ width: '100%' }} onClick={onOpenSettings}>
                <Globe size={16} />
                去设置注册
              </button>
            </div>
          </>
        )}
      </div>

      <div className="m-card">
        <div className="m-card-title">Agent 名片</div>
        <div className="provider-form">
          <input
            className="m-input"
            placeholder="名称"
            value={card.name}
            onChange={(event) => setCard({ ...card, name: event.target.value })}
          />
          <textarea
            className="m-input"
            placeholder="简介：这个 Agent 擅长什么"
            rows={3}
            value={card.bio}
            onChange={(event) => setCard({ ...card, bio: event.target.value })}
          />
          <input
            className="m-input"
            placeholder="能力标签，顿号/逗号分隔（如：笔记检索、联网搜索）"
            value={skillsText}
            onChange={(event) => setSkillsText(event.target.value)}
          />
        </div>
        {card.skills.length > 0 ? (
          <div className="theme-row" style={{ paddingTop: 4 }}>
            {card.skills.map((skill) => (
              <span key={skill} className="hub-skill-chip">{skill}</span>
            ))}
          </div>
        ) : null}
        <div style={{ padding: '12px 0 0' }}>
          <button className="m-btn-ghost" style={{ width: '100%' }} disabled={busy} onClick={doSave}>
            <Check size={16} />
            保存名片
          </button>
        </div>
      </div>

      <div className="m-card">
        <div className="m-card-title">AI 服务</div>
        <div className="card-hint" style={{ textAlign: 'left' }}>
          对话模型、联网搜索与 API Key 均在设置中配置（仅存本机，不上云）
        </div>
        <div style={{ padding: '12px 0 0' }}>
          <button className="m-btn-ghost" style={{ width: '100%' }} onClick={onOpenSettings}>
            <Settings size={16} />
            打开设置
          </button>
        </div>
      </div>

      {message ? (
        <div style={{ padding: '0 16px 12px', fontSize: 13, color: 'var(--muted-foreground)' }}>
          {message}
        </div>
      ) : null}
    </div>
  );
}

function MarketTab() {
  const [query, setQuery] = useState('');
  const [cards, setCards] = useState<MarketCard[] | null>(null);
  const [subs, setSubs] = useState<SubscriptionEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');

  const loadSubs = () => {
    api.marketSubscriptions().then(setSubs).catch(() => undefined);
  };
  useEffect(loadSubs, []);

  const doSearch = async () => {
    if (busy) return;
    setBusy(true);
    setMessage('搜索中…');
    try {
      const results = await api.marketSearch(query);
      setCards(results);
      setMessage(results.length === 0 ? '没有匹配的公开卡' : `找到 ${results.length} 张卡`);
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  const subAddresses = new Set(subs.map((entry) => entry.address));
  const toggle = async (address: string, displayName: string) => {
    try {
      const next = subAddresses.has(address)
        ? await api.marketUnsubscribe(address)
        : await api.marketSubscribe(address, displayName);
      setSubs(next);
    } catch (e) {
      setMessage(String(e));
    }
  };

  return (
    <div className="settings-list">
      <div className="hub-search-row">
        <input
          className="m-input"
          placeholder="搜索 Agent（名称/标签；留空看全部）"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') void doSearch();
          }}
        />
        <button className="m-icon-btn hub-search-btn" aria-label="搜索" disabled={busy} onClick={() => void doSearch()}>
          <Search size={18} />
        </button>
      </div>

      {message ? (
        <div style={{ padding: '0 16px 8px', fontSize: 13, color: 'var(--muted-foreground)' }}>
          {message}
        </div>
      ) : null}

      {subs.length > 0 ? (
        <div className="m-card">
          <div className="m-card-title">我的订阅</div>
          {subs.map((entry) => (
            <div key={entry.address} className="hub-card-row">
              <div className="hub-card-main">
                <div className="hub-card-title">{entry.displayName || entry.address}</div>
                <div className="hub-card-sub">{entry.address}</div>
              </div>
              <button className="m-icon-btn" aria-label="退订" onClick={() => void toggle(entry.address, entry.displayName)}>
                <UserMinus size={16} />
              </button>
            </div>
          ))}
        </div>
      ) : null}

      <div className="m-card">
        <div className="m-card-title">市场</div>
        {cards === null ? (
          <div className="m-empty">输入关键词或点搜索浏览全部公开卡</div>
        ) : cards.length === 0 ? (
          <div className="m-empty">没有匹配的公开卡</div>
        ) : (
          cards.map((cardItem) => (
            <div key={cardItem.address} className="hub-card-row">
              <div className="hub-card-main">
                <div className="hub-card-title">{cardItem.displayName}</div>
                <div className="hub-card-sub">{cardItem.address}</div>
                {cardTags(cardItem).length > 0 ? (
                  <div className="theme-row" style={{ paddingTop: 4 }}>
                    {cardTags(cardItem).map((tag) => (
                      <span key={tag} className="hub-skill-chip">{tag}</span>
                    ))}
                  </div>
                ) : null}
              </div>
              <button className="m-icon-btn" aria-label="订阅" onClick={() => void toggle(cardItem.address, cardItem.displayName)}>
                {subAddresses.has(cardItem.address) ? <Check size={17} /> : <UserPlus size={17} />}
              </button>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
