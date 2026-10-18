import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@flame';
import './styles.css';
import './simple.css';
import { App } from './App';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
