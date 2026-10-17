import type { SVGProps } from 'react';

type P = SVGProps<SVGSVGElement>;

const base = (d: React.ReactNode) =>
  function Icon(p: P) {
    return (
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.8}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        {...p}
      >
        {d}
      </svg>
    );
  };

export const IconChat = base(<path d="M4 5h16v11H8l-4 4z" />);
export const IconRuns = base(
  <>
    <path d="M4 6h10M4 12h16M4 18h7" />
    <circle cx="18" cy="6" r="2" />
    <circle cx="15" cy="18" r="2" />
  </>,
);
export const IconAgent = base(
  <>
    <rect x="5" y="8" width="14" height="11" rx="3" />
    <path d="M12 4v4M9 13h.01M15 13h.01M3 13h2M19 13h2" />
  </>,
);
export const IconModel = base(
  <>
    <path d="M12 3 4 7.5v9L12 21l8-4.5v-9z" />
    <path d="M4 7.5 12 12l8-4.5M12 12v9" />
  </>,
);
export const IconTool = base(<path d="M14.5 6.5a4 4 0 0 0-5.6 4.9L4 16.3V20h3.7l4.9-4.9a4 4 0 0 0 4.9-5.6l-2.6 2.6-2.4-.6-.6-2.4z" />);
export const IconUsage = base(<path d="M4 20V10M10 20V4M16 20v-7M22 20H2" />);
export const IconPlus = base(<path d="M12 5v14M5 12h14" />);
export const IconSend = base(<path d="M5 12h13M13 6l6 6-6 6" />);
export const IconStop = base(<rect x="7" y="7" width="10" height="10" rx="2" />);
export const IconTrash = base(<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />);
export const IconCheck = base(<path d="m5 12 5 5L20 7" />);
export const IconX = base(<path d="M6 6l12 12M18 6 6 18" />);
export const IconChevron = base(<path d="m9 6 6 6-6 6" />);
export const IconSpark = base(<path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M6 18l2.5-2.5M15.5 8.5 18 6" />);
export const IconBack = base(<path d="M15 6l-6 6 6 6" />);
export const IconKey = base(
  <>
    <circle cx="8" cy="15" r="4" />
    <path d="m11 12 9-9M17 6l3 3M14 9l2 2" />
  </>,
);
export const IconBook = base(<path d="M5 4h10a4 4 0 0 1 4 4v12H9a4 4 0 0 1-4-4zM5 16a4 4 0 0 1 4-4h10" />);
export const IconHand = base(<path d="M8 13V5.5a1.5 1.5 0 0 1 3 0V12M11 11V4a1.5 1.5 0 0 1 3 0v7M14 11V5.5a1.5 1.5 0 0 1 3 0V14a7 7 0 0 1-7 7h-.5a6 6 0 0 1-5.2-3l-2-3.6a1.5 1.5 0 0 1 2.6-1.5L8 15" />);
export const IconStack = base(
  <>
    <ellipse cx="12" cy="6" rx="7" ry="3" />
    <path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" />
  </>,
);
export const IconBrain = base(
  <path d="M9 4a3 3 0 0 0-3 3 3 3 0 0 0-2 5 3 3 0 0 0 2 5 3 3 0 0 0 3 3 3 3 0 0 0 3-3V7a3 3 0 0 0-3-3ZM15 4a3 3 0 0 1 3 3 3 3 0 0 1 2 5 3 3 0 0 1-2 5 3 3 0 0 1-3 3 3 3 0 0 1-3-3" />,
);
export const IconBolt = base(<path d="M13 3 5 14h6l-1 7 8-11h-6z" />);
export const IconSearch = base(
  <>
    <circle cx="11" cy="11" r="6" />
    <path d="m20 20-4.5-4.5" />
  </>,
);
export const IconFlow = base(
  <>
    <rect x="3" y="4" width="6" height="5" rx="1.5" />
    <rect x="15" y="4" width="6" height="5" rx="1.5" />
    <rect x="9" y="15" width="6" height="5" rx="1.5" />
    <path d="M6 9v2a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V9M12 13v2" />
  </>,
);
export const IconMic = base(
  <>
    <rect x="9" y="3" width="6" height="11" rx="3" />
    <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
  </>,
);
export const IconSpeaker = base(<path d="M4 9h4l5-4v14l-5-4H4zM16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12" />);
export const IconImage = base(
  <>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <circle cx="9" cy="10" r="2" />
    <path d="m21 16-5-5-9 9" />
  </>,
);
export const IconClock = base(
  <>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5l3 2" />
  </>,
);
export const IconPlug = base(<path d="M9 3v5M15 3v5M7 8h10v3a5 5 0 0 1-10 0zM12 16v5" />);
