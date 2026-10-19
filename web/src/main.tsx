import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './index.css';
import { applyTheme } from './prefs';
import { TooltipProvider } from '@/components/ui/tooltip';
import { App } from './App';

// Set light/dark before the first paint, so the page does not flash.
applyTheme();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <TooltipProvider delayDuration={400}>
      <App />
    </TooltipProvider>
  </StrictMode>,
);
