/** Type declarations for a-da agent-coding extension packages. */
export type EventAction = "observe" | "note" | "replace" | "deny" | "abort";

export interface ExtensionEventContext {
	readonly threadId: string;
	readonly turnId: string;
	readonly callId: string | null;
	readonly pluginId: string;
	/** Unified event name (`event_*`). */
	readonly event: string;
	readonly workspace: string;
	readonly timeoutMs: number;
	registerTool(tool: ToolDefinition): { ok: boolean; reason?: string };
	registerSystemPrompt(prompt: SystemPromptDefinition): { ok: boolean; reason?: string };
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

/** UI action intent; `read` forbids write-mode actions. */
export type ActionMode = "read" | "write";

export interface ActionContext {
	readonly workspace: string;
	readonly caller: "web";
	/** Host clock, epoch milliseconds. */
	readonly now: number;
}

export interface ActionDefinition {
	name: string;
	label?: string;
	description: string;
	mode?: ActionMode;
	parameters?: Record<string, unknown>;
	execute: (
		input: Record<string, unknown>,
		ctx: ActionContext,
	) => unknown | Promise<unknown>;
}

/** Dynamic HTTP route method; `*` matches every method. */
export type RouteMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "*";

export interface RouteRequest {
	readonly method: string;
	/** Path under `/ext/{pluginId}/route`, always starts with `/`. */
	readonly path: string;
	/** Matched `:param` / `*rest` segments. */
	readonly params: Record<string, string>;
	/** Repeated query keys become string arrays. */
	readonly query: Record<string, string | string[]>;
	/** Sanitized headers; `cookie` / `authorization` are never forwarded. */
	readonly headers: Record<string, string>;
	readonly body: unknown;
	readonly bodyKind: "empty" | "json" | "form" | "text" | "base64";
	readonly workspace: string;
	readonly threadId: string | null;
	readonly now: number;
}

export interface RouteContext {
	readonly workspace: string;
	readonly now: number;
	readonly threadId: string | null;
}

export interface RouteDefinition {
	name?: string;
	method?: RouteMethod;
	/** Static / `:param` / `*rest` path; `*rest` must be the last segment. */
	path: string;
	handler: (req: RouteRequest, ctx: RouteContext) => unknown | Promise<unknown>;
}

/** Tagged HTTP response; prefer the helpers on `ada` over building this by hand. */
export interface HttpResponse {
	status?: number;
	headers?: Record<string, string | string[]>;
	body?: unknown;
	bodyKind?: "text" | "base64";
	contentType?: string;
}

export interface RouteResponseHelpers {
	response(options: HttpResponse): HttpResponse;
	json(data: unknown, status?: number, headers?: Record<string, string | string[]>): HttpResponse;
	text(body: string, status?: number, headers?: Record<string, string | string[]>): HttpResponse;
	html(body: string, status?: number, headers?: Record<string, string | string[]>): HttpResponse;
	redirect(location: string, status?: number): HttpResponse;
}

/** Object-form event declaration (`export default { events: [...] }`). */
export interface EventDefinition {
	/** Unified event name (`event_*`). */
	event: string;
	actions?: EventAction[];
	order?: number;
	handler: (event: Record<string, unknown>, ctx: ExtensionEventContext) => unknown | Promise<unknown>;
}

export interface ExtensionMetadataGroup {
	count: number;
	/** Exact registered names/ids/events, validated against the factory output. */
	names: string[];
}

export interface ExtensionMetadata {
	name: string;
	description: string;
	tools: ExtensionMetadataGroup;
	systemPrompt: ExtensionMetadataGroup;
	/** Skills are directories (`skills/<name>/SKILL.md`), so this group is declarative. */
	skills: ExtensionMetadataGroup;
	events: ExtensionMetadataGroup;
	/** Optional; required when the factory registers actions. */
	actions?: ExtensionMetadataGroup;
	/** Optional; when present, validated against the routes registered by `ada.route`. */
	routes?: ExtensionMetadataGroup;
}

export interface ExtensionAPI extends RouteResponseHelpers {
	readonly workspace: string;
	/** Required package metadata; inspect validates it against actual registrations. */
	metadata?: ExtensionMetadata;
	trace(message: string): void;
	registerTool(tool: ToolDefinition): void;
	/** UI-only action; never enters the model tool catalog. */
	registerAction(action: ActionDefinition): void;
	/** Dynamic HTTP route; adding one needs no host change. */
	route(route: RouteDefinition): void;
	/** Alias for `route`. */
	registerRoute(route: RouteDefinition): void;
	registerSystemPrompt(prompt: SystemPromptDefinition): void;
	/** Compatibility alias for registerSystemPrompt. */
	registerPrompt(prompt: SystemPromptDefinition): void;
	/**
	 * The only event registration entrypoint. `event` must be a unified `event_*`
	 * name; object-form `events` declarations are also accepted.
	 */
	on(
		event: string,
		handler: (event: Record<string, unknown>, ctx: ExtensionEventContext) => unknown | Promise<unknown>,
		options?: { actions?: EventAction[]; order?: number },
	): void;
}

export type ExtensionFactory = (ada: ExtensionAPI) => void | Promise<void>;

/** Package entrypoint uses a pi-style default factory; metadata is assigned on `ada`. */
export default function extension(ada: ExtensionAPI): void | Promise<void>;
