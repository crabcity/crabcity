import { updateVoiceMetrics, recordVoiceTranscription, recordVoiceError } from '$lib/stores/metrics';

export type VoiceBackend = 'prompt-api' | 'web-speech' | 'none';

export interface VoiceSessionCallbacks {
	onInterim: (text: string) => void;
	onFinal: (text: string) => void;
	onError: (err: string) => void;
	onStateChange: (state: 'listening' | 'transcribing' | 'idle') => void;
}

export interface VoiceSession {
	start(): void;
	stop(): void;
	destroy(): void;
	backend: VoiceBackend;
}

export async function detectVoiceBackend(): Promise<VoiceBackend> {
	// Try Prompt API with audio support first
	if (typeof LanguageModel !== 'undefined') {
		try {
			const availability = await LanguageModel.availability({
				expectedInputs: [{ type: 'audio' }],
			});
			if (availability === 'available' || availability === 'downloadable') {
				return 'prompt-api';
			}
		} catch {
			// Prompt API present but audio not supported — fall through
		}
	}

	// Fall back to Web Speech API
	const SpeechRecognitionCtor =
		typeof window !== 'undefined'
			? window.SpeechRecognition || window.webkitSpeechRecognition
			: undefined;
	if (SpeechRecognitionCtor) {
		return 'web-speech';
	}

	return 'none';
}

export function createVoiceSession(
	backend: VoiceBackend,
	callbacks: VoiceSessionCallbacks,
): VoiceSession {
	updateVoiceMetrics({ backend });
	if (backend === 'prompt-api') {
		return createPromptApiSession(callbacks);
	}
	if (backend === 'web-speech') {
		return createWebSpeechSession(callbacks);
	}
	return { start() {}, stop() {}, destroy() {}, backend: 'none' };
}

// ---------------------------------------------------------------------------
// Prompt API backend — records audio, then batch-transcribes via Gemini Nano
// ---------------------------------------------------------------------------

function createPromptApiSession(cb: VoiceSessionCallbacks): VoiceSession {
	let session: LanguageModel | null = null;
	let mediaStream: MediaStream | null = null;
	let recorder: MediaRecorder | null = null;
	let chunks: Blob[] = [];
	let active = false;
	let destroyed = false;
	let transcribeStart = 0;

	// Eagerly create the language model session
	LanguageModel.create({ expectedInputs: [{ type: 'audio' }] })
		.then((s) => {
			if (destroyed) {
				s.destroy();
				return;
			}
			session = s;
		})
		.catch((err) => {
			cb.onError(`Failed to create Prompt API session: ${err}`);
			recordVoiceError();
		});

	return {
		backend: 'prompt-api',

		start() {
			if (active || destroyed) return;
			active = true;
			chunks = [];

			navigator.mediaDevices
				.getUserMedia({ audio: true })
				.then((stream) => {
					if (!active || destroyed) {
						stream.getTracks().forEach((t) => t.stop());
						return;
					}
					mediaStream = stream;
					recorder = new MediaRecorder(stream);
					recorder.ondataavailable = (e) => {
						if (e.data.size > 0) chunks.push(e.data);
					};
					recorder.start();
					cb.onStateChange('listening');
					updateVoiceMetrics({ state: 'listening' });
				})
				.catch((err) => {
					active = false;
					cb.onError(`Microphone access denied: ${err}`);
					cb.onStateChange('idle');
					recordVoiceError();
				});
		},

		stop() {
			if (!active || destroyed) return;
			active = false;

			if (recorder && recorder.state !== 'inactive') {
				recorder.onstop = async () => {
					// Stop mic tracks
					mediaStream?.getTracks().forEach((t) => t.stop());
					mediaStream = null;
					recorder = null;

					if (chunks.length === 0) {
						cb.onStateChange('idle');
						updateVoiceMetrics({ state: 'idle' });
						return;
					}

					cb.onStateChange('transcribing');
					updateVoiceMetrics({ state: 'transcribing' });
					transcribeStart = performance.now();

					try {
						const blob = new Blob(chunks, { type: 'audio/webm' });
						const arrayBuffer = await blob.arrayBuffer();
						const audioCtx = new AudioContext();
						const audioBuffer = await audioCtx.decodeAudioData(arrayBuffer);
						await audioCtx.close();

						if (!session) {
							throw new Error('Prompt API session not ready');
						}

						const response = await session.prompt([
							{
								role: 'user',
								content: [
									{
										type: 'text',
										value:
											'Transcribe the following audio exactly as spoken. Output only the transcribed text, nothing else.',
									},
									{ type: 'audio', value: audioBuffer },
								],
							},
						]);

						const elapsed = Math.round(performance.now() - transcribeStart);
						cb.onFinal(response.trim());
						recordVoiceTranscription(elapsed);
					} catch (err) {
						cb.onError(`Transcription failed: ${err}`);
						recordVoiceError();
					} finally {
						cb.onStateChange('idle');
						updateVoiceMetrics({ state: 'idle' });
					}
				};
				recorder.stop();
			} else {
				mediaStream?.getTracks().forEach((t) => t.stop());
				mediaStream = null;
				recorder = null;
				cb.onStateChange('idle');
				updateVoiceMetrics({ state: 'idle' });
			}
		},

		destroy() {
			destroyed = true;
			active = false;
			if (recorder && recorder.state !== 'inactive') {
				recorder.stop();
			}
			mediaStream?.getTracks().forEach((t) => t.stop());
			mediaStream = null;
			recorder = null;
			session?.destroy();
			session = null;
		},
	};
}

// ---------------------------------------------------------------------------
// Web Speech API backend — streaming recognition via browser/OS speech engine
// ---------------------------------------------------------------------------

function createWebSpeechSession(cb: VoiceSessionCallbacks): VoiceSession {
	const SpeechRecognitionCtor = window.SpeechRecognition || window.webkitSpeechRecognition;
	if (!SpeechRecognitionCtor) throw new Error('Web Speech API not available');
	const recognition = new SpeechRecognitionCtor();
	recognition.continuous = true;
	recognition.interimResults = true;
	recognition.lang = 'en-US';

	recognition.onresult = (event: SpeechRecognitionEvent) => {
		const transcript = Array.from(event.results)
			.map((result) => result[0].transcript)
			.join(' ');

		const lastResult = event.results[event.results.length - 1];
		if (lastResult.isFinal) {
			cb.onFinal(transcript);
			recordVoiceTranscription();
		} else {
			cb.onInterim(transcript);
		}
	};

	recognition.onerror = (event: SpeechRecognitionErrorEvent) => {
		console.error('Speech recognition error:', event.error);
		cb.onError(event.error);
		cb.onStateChange('idle');
		recordVoiceError();
	};

	recognition.onend = () => {
		cb.onStateChange('idle');
		updateVoiceMetrics({ state: 'idle' });
	};

	return {
		backend: 'web-speech',

		start() {
			recognition.start();
			cb.onStateChange('listening');
			updateVoiceMetrics({ state: 'listening' });
		},

		stop() {
			recognition.stop();
			// onend callback will fire onStateChange('idle')
		},

		destroy() {
			recognition.abort();
		},
	};
}
