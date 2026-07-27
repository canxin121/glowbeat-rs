import Foundation
import GlowBeatNative

public enum GlowBeatError: Error {
    case invalidConfiguration
    case analysisFailed
    case invalidScore
    case nativeFailure(Int32)
}

public final class GlowBeatEngine {
    private var handle: OpaquePointer?

    public init(sampleRate: UInt32, channels: UInt32, lightCapacity: UInt32) throws {
        var config = glowbeat_default_config()
        config.sample_rate = sampleRate
        config.channels = channels
        config.light_capacity = lightCapacity
        guard let created = glowbeat_engine_create(config) else {
            throw GlowBeatError.invalidConfiguration
        }
        handle = created
    }

    deinit {
        if let handle { glowbeat_engine_destroy(handle) }
    }

    @discardableResult
    public func push(_ pcm: UnsafePointer<Float>?, count: Int) throws -> GlowbeatFfiReport {
        guard let handle else { throw GlowBeatError.invalidConfiguration }
        var report = GlowbeatFfiReport()
        let code = glowbeat_engine_push_f32(
            handle, pcm, count, &report)
        guard code == 0 else { throw GlowBeatError.nativeFailure(code) }
        return report
    }

    public func render(
        sampleIndex: UInt64,
        points: UnsafePointer<GlowbeatLightPoint>?,
        count: Int,
        output: UnsafeMutablePointer<GlowbeatRgba>?
    ) throws {
        guard let handle else { throw GlowBeatError.invalidConfiguration }
        let code = glowbeat_engine_render(
            handle, sampleIndex, points, count, output)
        guard code == 0 else { throw GlowBeatError.nativeFailure(code) }
    }
}

public final class GlowBeatScore {
    fileprivate var handle: OpaquePointer?

    private init(handle: OpaquePointer) { self.handle = handle }

    deinit {
        if let handle { glowbeat_score_destroy(handle) }
    }

    public static func analyze(
        pcm: UnsafePointer<Float>,
        count: Int,
        sampleRate: UInt32,
        channels: UInt32
    ) throws -> GlowBeatScore {
        guard let native = glowbeat_score_analyze_f32(pcm, count, sampleRate, channels) else {
            throw GlowBeatError.analysisFailed
        }
        return GlowBeatScore(handle: native)
    }

    public static func deserialize(_ bytes: Data) throws -> GlowBeatScore {
        let native: OpaquePointer? = bytes.withUnsafeBytes { raw in
            guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return nil }
            return glowbeat_score_deserialize(base, raw.count)
        }
        guard let native else { throw GlowBeatError.invalidScore }
        return GlowBeatScore(handle: native)
    }

    public func serialize() throws -> Data {
        guard let handle else { throw GlowBeatError.invalidScore }
        var size = 0
        var code = glowbeat_score_serialized_size(handle, &size)
        guard code == 0 else { throw GlowBeatError.nativeFailure(code) }
        var data = Data(count: size)
        var written = 0
        code = data.withUnsafeMutableBytes { raw in
            glowbeat_score_serialize(
                handle,
                raw.baseAddress?.assumingMemoryBound(to: UInt8.self),
                raw.count,
                &written)
        }
        guard code == 0, written == size else { throw GlowBeatError.nativeFailure(code) }
        return data
    }

    public func makePlayer(lightCapacity: UInt32) throws -> GlowBeatPlayer {
        guard let handle else { throw GlowBeatError.invalidScore }
        var config = glowbeat_default_config()
        config.light_capacity = lightCapacity
        guard let native = glowbeat_player_create(handle, config) else {
            throw GlowBeatError.invalidConfiguration
        }
        return GlowBeatPlayer(handle: native)
    }
}

public final class GlowBeatPlayer {
    private var handle: OpaquePointer?

    fileprivate init(handle: OpaquePointer) { self.handle = handle }

    deinit {
        if let handle { glowbeat_player_destroy(handle) }
    }

    public func seek(sampleIndex: UInt64) throws {
        guard let handle else { throw GlowBeatError.invalidConfiguration }
        let code = glowbeat_player_seek(handle, sampleIndex)
        guard code == 0 else { throw GlowBeatError.nativeFailure(code) }
    }

    public func render(
        sampleIndex: UInt64,
        points: UnsafePointer<GlowbeatLightPoint>?,
        count: Int,
        output: UnsafeMutablePointer<GlowbeatRgba>?
    ) throws {
        guard let handle else { throw GlowBeatError.invalidConfiguration }
        let code = glowbeat_player_render(
            handle, sampleIndex, points, count, output)
        guard code == 0 else { throw GlowBeatError.nativeFailure(code) }
    }
}
