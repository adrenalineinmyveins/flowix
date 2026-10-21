import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, SearchX } from 'lucide-react';
import { api, NotebookInfo, SearchHit } from './api';
import { OpenNote } from './App';

interface Props {
  notebooks: NotebookInfo[];
  onBack: () => void;
  onOpenNote: (note: OpenNote) => void;
}

/** 关键词高亮：大小写不敏感切分，命中片段染 --brand 选区色。 */
function Highlight({ text, query }: { text: string; query: string }) {
  const q = query.trim();
  if (!q) return <>{text}</>;
  const parts = text.split(new RegExp(`(${q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'gi'));
  return (
    <>
      {parts.map((part, i) =>
        part.toLowerCase() === q.toLowerCase() ? <mark key={i}>{part}</mark> : part,
      )}
    </>
  );
}

export function SearchScreen({ notebooks, onBack, onOpenNote }: Props) {
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  const timer = useRef<number>(0);

  // 300ms 防抖实时搜索
  useEffect(() => {
    window.clearTimeout(timer.current);
    const q = query.trim();
    if (!q) {
      setHits(null);
      setSearching(false);
      return;
    }
    setSearching(true);
    timer.current = window.setTimeout(async () => {
      try {
        setHits(await api.search(q));
      } catch {
        setHits([]);
      } finally {
        setSearching(false);
      }
    }, 300);
    return () => window.clearTimeout(timer.current);
  }, [query]);

  const notebookName = (id: string) => notebooks.find((n) => n.id === id)?.name || id;

  return (
    <div className="screen">
      <div className="search-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowLeft />
        </button>
        <input
          className="m-input"
          placeholder="搜索全部笔记"
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      <div className="screen-scroll">
        {query.trim() === '' && <div className="search-hint">输入关键词，搜索全部已同步笔记</div>}
        {searching && <div className="search-hint">搜索中…</div>}
        {hits !== null && !searching && hits.length === 0 && (
          <div className="m-empty">
            <SearchX />
            未找到笔记
          </div>
        )}
        {hits?.map((hit) => (
          <div
            key={`${hit.notebookId}/${hit.relativePath}`}
            className="search-result"
            onClick={() => onOpenNote({ notebookId: hit.notebookId, relativePath: hit.relativePath })}
          >
            <div className="title">
              <Highlight text={hit.title || hit.relativePath} query={query} />
            </div>
            {hit.snippet && (
              <div className="snippet">
                <Highlight text={hit.snippet} query={query} />
              </div>
            )}
            <div className="notebook">{notebookName(hit.notebookId)}</div>
          </div>
        ))}
      </div>
    </div>
  );
}
