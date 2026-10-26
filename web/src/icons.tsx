// The dashboard's icons, from Lucide (the icon set the shadcn components use).
// Named for what they mean in gai, so pages don't depend on Lucide's names.

import {
  ArrowLeftIcon,
  ArrowRightIcon,
  BookOpenIcon,
  BotIcon,
  BrainIcon,
  ChartColumnIcon,
  CheckIcon,
  ChevronRightIcon,
  ClockIcon,
  CpuIcon,
  HandIcon,
  HistoryIcon,
  ImageIcon,
  KeyRoundIcon,
  LayersIcon,
  DatabaseIcon,
  MessageSquareIcon,
  MicIcon,
  MoonIcon,
  PaperclipIcon,
  PlugIcon,
  PlusIcon,
  SearchIcon,
  SettingsIcon,
  SparklesIcon,
  SquareIcon,
  SunIcon,
  Trash2Icon,
  UploadIcon,
  Volume2Icon,
  WorkflowIcon,
  WrenchIcon,
  XIcon,
  ZapIcon,
  type LucideIcon,
  type LucideProps,
} from 'lucide-react';

// 16px and a slightly lighter stroke than Lucide's default, to sit with
// 14px text. Inside shadcn components their own CSS sets the size.
const icon = (I: LucideIcon) =>
  function Icon(p: LucideProps) {
    return <I size={16} strokeWidth={1.8} aria-hidden="true" {...p} />;
  };

export const IconChat = icon(MessageSquareIcon);
export const IconRuns = icon(HistoryIcon);
export const IconAgent = icon(BotIcon);
export const IconModel = icon(CpuIcon);
export const IconTool = icon(WrenchIcon);
export const IconUsage = icon(ChartColumnIcon);
export const IconPlus = icon(PlusIcon);
export const IconSend = icon(ArrowRightIcon);
export const IconStop = icon(SquareIcon);
export const IconTrash = icon(Trash2Icon);
export const IconCheck = icon(CheckIcon);
export const IconX = icon(XIcon);
export const IconChevron = icon(ChevronRightIcon);
export const IconSpark = icon(SparklesIcon);
export const IconBack = icon(ArrowLeftIcon);
export const IconKey = icon(KeyRoundIcon);
export const IconBook = icon(BookOpenIcon);
export const IconHand = icon(HandIcon);
export const IconStack = icon(LayersIcon);
export const IconData = icon(DatabaseIcon);
export const IconBrain = icon(BrainIcon);
export const IconBolt = icon(ZapIcon);
export const IconSearch = icon(SearchIcon);
export const IconFlow = icon(WorkflowIcon);
export const IconMic = icon(MicIcon);
export const IconSpeaker = icon(Volume2Icon);
export const IconImage = icon(ImageIcon);
export const IconClock = icon(ClockIcon);
export const IconPlug = icon(PlugIcon);
export const IconGear = icon(SettingsIcon);
export const IconSun = icon(SunIcon);
export const IconMoon = icon(MoonIcon);
export const IconClip = icon(PaperclipIcon);
export const IconUpload = icon(UploadIcon);
