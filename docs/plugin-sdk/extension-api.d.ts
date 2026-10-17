/** Type declarations for a-da agent-coding extension packages. */
export type HookAction = "observe" | "note" | "replace" | "deny" | "abort";

export interface ExtensionHookContext {
	readonly threadId: string;
	readonly turnId: string;
	readonly callId: string | null;
	readonly pluginId: string;
	readonly point: string;
	readonly workspace: string;
	readonly timeoutMs: number;
	registerTool(tool: ToolDefinition): { ok: boolean; reason?: string };
	registerSystemPrompt(prompt: SystemPromptDefinition): { ok: boolean; reason?: string };
	registerSkill(skill: SkillDefinition): { ok: boolean; reason?: string };
	registerHook(hook: HookDefinition): { ok: boolean; reason?: string };
}

export interface ToolDefinition {
	name: string;
	description: string;
	parameters: Record<string, unknown>;
	execute: (args: Record<string, unknown>) => unknown | Promise<unknown>;
}

export interface SystemPromptDefinition {
	id: string;
	content: string;
}

export interface SkillDefinition {
	name: string;
	description: string;
	/** Relative to the plugin package root or `.ada/agent-coding/skills/`. */
	path: string;
	body?: string;
}

export interface HookDefinition {
	point: string;
	actions?: HookAction[];
	order?: number;
	handlerExport?: string;
	handler?: (event: Record<string, unknown>, ctx: ExtensionHookContext) => unknown | Promise<unknown>;
}

export interface ExtensionMetadataGroup {
	count: number;
	/** Exact registered names/ids/points, validated against the factory output. */
	names: string[];
}

export interface ExtensionMetadata {
	name: string;
	description: string;
	tools: ExtensionMetadataGroup;
	systemPrompt: ExtensionMetadataGroup;
	skills: ExtensionMetadataGroup;
	hooks: ExtensionMetadataGroup;
}

export interface ExtensionAPI {
	readonly workspace: string;
	/** Required package metadata; inspect validates it against actual registrations. */
	metadata?: ExtensionMetadata;
	trace(message: string): void;
	registerTool(tool: ToolDefinition): void;
	registerSystemPrompt(prompt: SystemPromptDefinition): void;
	/** Compatibility alias for registerSystemPrompt. */
	registerPrompt(prompt: SystemPromptDefinition): void;
	registerSkill(skill: SkillDefinition): void;
	registerHook(hook: HookDefinition): void;
	on(
		event: string,
		handler: (event: Record<string, unknown>, ctx: ExtensionHookContext) => unknown | Promise<unknown>,
		options?: { actions?: HookAction[]; order?: number },
	): void;
}

export type ExtensionFactory = (ada: ExtensionAPI) => void | Promise<void>;

/** Package entrypoint uses a pi-style default factory; metadata is assigned on `ada`. */
export default function extension(ada: ExtensionAPI): void | Promise<void>;
