declare module 'obsidian' {
  export interface EventRef {}

  export interface TAbstractFile {
    vault: Vault;
    path: string;
    name: string;
    parent: TFolder | null;
  }

  export interface FileStats {
    ctime: number;
    mtime: number;
    size: number;
  }

  export interface TFile extends TAbstractFile {
    stat: FileStats;
    basename: string;
    extension: string;
  }

  export interface TFolder extends TAbstractFile {
    children: TAbstractFile[];
    isRoot(): boolean;
  }

  export interface Vault {
    getMarkdownFiles(): TFile[];
    getAbstractFileByPath(path: string): TAbstractFile | null;
    read(file: TFile): Promise<string>;
    cachedRead(file: TFile): Promise<string>;
    create(path: string, data: string): Promise<TFile>;
    modify(file: TFile, data: string): Promise<void>;
    createFolder(path: string): Promise<TFolder>;
    on(name: 'create' | 'modify' | 'delete', callback: (file: TAbstractFile) => unknown): EventRef;
    on(name: 'rename', callback: (file: TAbstractFile, oldPath: string) => unknown): EventRef;
    offref(ref: EventRef): void;
  }

  export interface WorkspaceLeaf {
    view: ItemView;
    setViewState(viewState: {type: string; active?: boolean}): Promise<void>;
  }

  export interface Workspace {
    onLayoutReady(callback: () => unknown): void;
    getLeavesOfType(viewType: string): WorkspaceLeaf[];
    getRightLeaf(split: boolean): WorkspaceLeaf | null;
    revealLeaf(leaf: WorkspaceLeaf): Promise<void> | void;
  }

  export interface App {
    vault: Vault;
    workspace: Workspace;
  }

  export interface PluginManifest {
    id: string;
    name: string;
    version: string;
    minAppVersion: string;
    description?: string;
    author?: string;
    authorUrl?: string;
    isDesktopOnly?: boolean;
  }

  export interface Command {
    id: string;
    name: string;
    callback?: () => unknown;
    checkCallback?: (checking: boolean) => boolean | void;
  }

  export class ItemView {
    app: App;
    leaf: WorkspaceLeaf;
    containerEl: HTMLElement;
    contentEl: HTMLElement;
    constructor(leaf: WorkspaceLeaf);
    getViewType(): string;
    getDisplayText(): string;
    onOpen(): Promise<void>;
    onClose(): Promise<void>;
  }

  export class Plugin {
    app: App;
    manifest: PluginManifest;
    constructor(app: App, manifest: PluginManifest);
    onload(): Promise<void> | void;
    onunload(): void;
    addStatusBarItem(): HTMLElement;
    addCommand(command: Command): Command;
    addSettingTab(settingTab: PluginSettingTab): void;
    registerEvent(eventRef: EventRef): void;
    registerInterval(id: number): number;
    registerView(type: string, viewCreator: (leaf: WorkspaceLeaf) => ItemView): void;
    loadData(): Promise<unknown>;
    saveData(data: unknown): Promise<void>;
  }

  export class PluginSettingTab {
    app: App;
    containerEl: HTMLElement;
    constructor(app: App, plugin: Plugin);
    display(): void;
    hide(): void;
  }

  export interface TextComponent {
    inputEl: HTMLInputElement;
    setPlaceholder(placeholder: string): this;
    setValue(value: string): this;
    onChange(callback: (value: string) => unknown): this;
  }

  export interface ToggleComponent {
    setValue(on: boolean): this;
    onChange(callback: (value: boolean) => unknown): this;
  }

  export interface SliderComponent {
    setLimits(min: number, max: number, step: number | 'any'): this;
    setValue(value: number): this;
    setDynamicTooltip(): this;
    onChange(callback: (value: number) => unknown): this;
  }

  export interface ButtonComponent {
    setButtonText(name: string): this;
    setCta(): this;
    setDisabled(disabled: boolean): this;
    onClick(callback: (evt: MouseEvent) => unknown): this;
  }

  export class Setting {
    settingEl: HTMLElement;
    constructor(containerEl: HTMLElement);
    setName(name: string): this;
    setDesc(desc: string): this;
    setHeading(): this;
    addText(cb: (component: TextComponent) => unknown): this;
    addToggle(cb: (component: ToggleComponent) => unknown): this;
    addSlider(cb: (component: SliderComponent) => unknown): this;
    addButton(cb: (component: ButtonComponent) => unknown): this;
  }

  export class Notice {
    constructor(message: string, duration?: number);
    setMessage(message: string): this;
    hide(): void;
  }

  export interface RequestUrlParam {
    url: string;
    method?: string;
    headers?: Record<string, string>;
    body?: string;
    throw?: boolean;
  }

  export interface RequestUrlResponse {
    status: number;
    headers: Record<string, string>;
    text: string;
    json: unknown;
  }

  export function requestUrl(request: RequestUrlParam | string): Promise<RequestUrlResponse>;
  export function normalizePath(path: string): string;
}
