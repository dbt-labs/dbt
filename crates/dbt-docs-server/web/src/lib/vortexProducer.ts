import {
  create as bufCreate,
  type DescMessage,
  type MessageShape,
  toBinary as bufToBinary,
} from '@bufbuild/protobuf';
import { AnySchema, timestampNow } from '@bufbuild/protobuf/wkt';
import { v4 as uuidv4 } from 'uuid';

import aboutProto from '../proto/about.json';
import {
  type VortexMessage,
  type VortexMessageBatch,
  VortexMessageBatchSchema,
  VortexMessageSchema,
} from '../proto/dbtlabs/proto/v1/events/vortex_pb';
import env from './vortexEnv';

const VORTEX_CLIENT_VERSION = '0.3.0';

/**
 * Base class for all VortexProducerClient errors.
 */
export class VortexProducerError extends Error {}

export class FailedToSendMessageError extends VortexProducerError {
  public status: number;
  public response: string;

  constructor(status: number, response: string) {
    super(
      `Failed to send message to Vortex service. status=${status.toString()}: ${response}`,
    );
    this.status = status;
    this.response = response;
  }
}

export class TimeoutError extends VortexProducerError {
  public deadlineSeconds: number;

  constructor(deadlineSeconds: number) {
    super(`Timeout deadline exceeded (${deadlineSeconds.toString()}s).`);
    this.deadlineSeconds = deadlineSeconds;
  }
}

/**
 * Logs messages to some sink.
 */
export interface Logger {
  debug(msg: string): void;
  info(msg: string): void;
  warn(msg: string): void;
  error(msg: string): void;
}

export type ErrorMode = 'log-and-continue' | 'log-and-throw';

export const URL_PRODUCTION = 'https://p.vx.dbt.com';

export const URL_STAGING = 'https://staging.vx.dbt.com';

/**
 * Config for a VortexProducerClient
 */
export type VortexProducerClientConfig = {
  /**
   * Whether the client is enabled or not. Setting this to false will make it silently drop
   * all subsequent logProto and flush calls.
   */
  enabled: boolean;

  /**
   * Default error mode if not overwriten in called methods.
   */
  errorMode: ErrorMode;

  /**
   * Where to log things to.
   */
  logger: Logger;

  /**
   * The default timeout seoncds if not overwriten in called methods. If <= 0, will never timeout.
   */
  timeoutSeconds: number;

  /**
   * The maximum number of bytes to buffer before sending a message batch. Set this to -1 to instantly flush
   * every message without having to call `flush()` explicitly.
   */
  maxBatchBytes: number;

  /**
   * The base URL for the collector server.
   */
  collectorBaseUrl: string;

  /**
   * The path to POST protobufs to
   */
  collectorPath: string;
};

/**
 * A client for sending messages to Vortex Kafka topics.
 *
 * Although you can instantiate this class directly, it is recommended you use the `globalClient` singleton
 * that's exported by default from this module.
 */
export class VortexProducerClient {
  public static DEFAULT_CONFIG: VortexProducerClientConfig = {
    enabled: true,
    timeoutSeconds: -1,
    errorMode: 'log-and-continue',
    logger: console,
    maxBatchBytes: 1024 * 400, // 400kb
    collectorBaseUrl: env.PRODUCTION ? URL_PRODUCTION : URL_STAGING,
    collectorPath: '/v1/ingest/protobuf',
  };

  public readonly config: VortexProducerClientConfig;

  private _queue: VortexMessage[] = [];
  private _queueSizeBytes: number = 0;

  /*
   * The current queue size, in number of messages.
   */
  public get queueLength(): number {
    return this._queue.length;
  }

  constructor() {
    this.config = Object.create(
      VortexProducerClient.DEFAULT_CONFIG,
    ) as VortexProducerClientConfig;
  }

  /**
   * Change the configurations of this client
   */
  public configure(
    config: Partial<VortexProducerClientConfig>,
  ): VortexProducerClientConfig {
    for (const [key, value] of Object.entries(config)) {
      // @ts-expect-error Config is a Partial<VortexProducerClientConfig>, which means the `key` will
      // always exist in VortexProducerClientConfig.
      // `keyof VortexProducerClientConfig`
      this.config[key] = value;
    }
    return this.config;
  }

  /**
   * Possibly send a protobuf message to vortex.
   *
   * Since JavaScript is single-threaded, there is no worker thread that sends batches to the
   * server. Instead, this method is async, and most of the times it will simply append to the
   * end of the pending message buffer and return immediately without performing any IO.
   *
   * However, if the queue is large enough (i.e the length is larger than maxBatchBytes), this
   * method will flush the queue and send a request as well.
   *
   * @param {Desc} schema the protobuf schema descriptor
   * @param {MessageShape<Desc>} message the protobuf message
   * @param {ErrorMode} errorMode whether to simply log errors or throw them
   * @param {number} timeoutSeconds seconds before timeout in case it flushes,
   *     will use default if not provided
   *
   * @return {number} the number of messages that were flushed to the server, if any
   */
  async logProto<Desc extends DescMessage>(
    schema: Desc,
    message: MessageShape<Desc>,
    errorMode: ErrorMode | null = null,
    timeoutSeconds: number | null = null,
  ): Promise<number> {
    if (!this.config.enabled) {
      return 0;
    }

    const vortexMessage = bufCreate(VortexMessageSchema, {
      // NOTE: we're not using @bufbuild/protobuf/wkt/anyPack because that always uses the prefix
      // @type.googleapis.com/ for the typename
      any: bufCreate(AnySchema, {
        value: bufToBinary(schema, message),
        typeUrl: `/${message.$typeName}`,
      }),
      vortexEventCreatedAt: timestampNow(),
      // envelope fields set automatically by the client.
      vortexEventId: uuidv4(),
      vortexEventUrl: `dbt-labs/proto/${message.$typeName.replaceAll('.', '/')}`,
    });

    this._queue.push(vortexMessage);
    this._queueSizeBytes += bufToBinary(VortexMessageSchema, vortexMessage).length;

    if (this._queueSizeBytes > this.config.maxBatchBytes) {
      return await this.flush(timeoutSeconds, errorMode);
    }

    return 0;
  }

  /**
   * Flush all pending messages in the queue.
   *
   * To avoid concurrent access issues, this method will create a new queue before suspending.
   * This way, producers can call `logProto` while the flush task is running in the background without
   * interfering with the pending flush coroutine that might be suspended.
   *
   * @param {number} timeoutSeconds Maximum many seconds to wait before raising a TimeoutError. If <= 0, will wait forever.
   *     Will use default if not provided.
   * @return {number} the number of elements that were flushed.
   */
  async flush(
    timeoutSeconds: number | null = 10.0,
    errorMode: ErrorMode | null = null,
  ): Promise<number> {
    if (!this.config.enabled) {
      return 0;
    }

    errorMode = errorMode || this.config.errorMode;

    const toFlush = this._queue;
    this._queue = [];
    this._queueSizeBytes = 0;

    if (toFlush.length == 0) {
      return 0;
    }

    const now = timestampNow();
    for (const msg of toFlush) {
      msg.vortexClientSentAt = now;
    }

    const batch = bufCreate(VortexMessageBatchSchema, {
      requestId: uuidv4(),
      payload: toFlush,
    });

    try {
      return await this._sendBatch(batch, timeoutSeconds);
    } catch (err: unknown) {
      const errStr = err instanceof Error ? err.message : String(err);
      this.config.logger.error(`Error occurred while sending message batch: ${errStr}`);
      if (errorMode == 'log-and-throw') {
        throw err;
      }
      return 0;
    }
  }

  private _getPlatformHeader(): string {
    const clientName = 'vortex-client-typescript';
    const clientVersion = VORTEX_CLIENT_VERSION;

    const serviceName = env.SERVICE || clientName;
    const serviceVersion = env.VERSION || clientVersion;

    const protoLibrary = 'proto-typescript';
    const protoVersion = aboutProto.version;

    return `${serviceName}/${serviceVersion} ${clientName}/${clientVersion} ${protoLibrary}/${protoVersion}`;
  }

  private _sendBatch(
    batch: VortexMessageBatch,
    timeoutSeconds: number | null = null,
  ): Promise<number> {
    timeoutSeconds = timeoutSeconds || this.config.timeoutSeconds;

    const url = this.config.collectorBaseUrl + this.config.collectorPath;

    return new Promise((res, rej) => {
      fetch(url, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/vnd.google.protobuf',
          'X-Vortex-Client-Platform': this._getPlatformHeader(),
        },
        body: bufToBinary(VortexMessageBatchSchema, batch),
        signal: timeoutSeconds > 0 ? AbortSignal.timeout(timeoutSeconds * 1000) : null,
      })
        .then((response) => {
          if (response.status == 202) {
            res(batch.payload.length);
          } else {
            response
              .text()
              .then((body) => {
                rej(new FailedToSendMessageError(response.status, body));
              })
              .catch(() => {
                rej(
                  new FailedToSendMessageError(
                    response.status,
                    'Failed to download response payload.',
                  ),
                );
              });
          }
        })
        .catch((err: unknown) => {
          if (!(err instanceof Error)) {
            rej(new Error(String(err)));
            // NOTE: MDN recommends using err.name to check for AbortError instead of an `instanceof` check
            // See: https://developer.mozilla.org/en-US/docs/Web/API/AbortSignal#aborting_a_fetch_operation_with_a_timeout
          } else if (err.name === 'AbortError') {
            rej(new TimeoutError(timeoutSeconds));
          } else {
            rej(err);
          }
        });
    });
  }
}

export const globalClient = new VortexProducerClient();
export default globalClient;
