import type {ReactNode} from 'react';
import {TICKER_ITEMS} from '../../data/landing';

export function TickerBand(): ReactNode {
  const repeated = [...TICKER_ITEMS, ...TICKER_ITEMS];

  return (
    <div className="band" aria-hidden="true">
      <div className="frame ticker-wrap">
        <div className="ticker-track">
          {repeated.map((label, idx) => (
            <span key={`${label}-${idx}`} className="ticker-item">
              {label} <span className="ticker-sep">+</span>
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}
