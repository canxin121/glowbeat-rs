using System;
using System.Runtime.InteropServices;

namespace GlowBeat
{
    public static class Native
    {
#if IOS
        private const string Library = "__Internal";
#else
        // Let .NET/Mono add the platform-specific prefix and suffix:
        // glowbeat.dll, libglowbeat.dylib, or libglowbeat.so.
        private const string Library = "glowbeat";
#endif

        [StructLayout(LayoutKind.Sequential)]
        public struct Config
        {
            public UInt32 sample_rate;
            public UInt32 channels;
            public UInt32 light_capacity;
            public UInt32 output_mode;
            public float render_fps;
            public float max_brightness;
            public float output_gamma;
            public float hue_offset;
            public float amber_mix;
            public float saturation;
            public float beat_strength;
            public float accent_strength;
            public float spatial_strength;
            public float max_luma_step;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct Report
        {
            public UInt64 sample_frames;
            public UInt64 analysis_frames;
            public UInt64 sample_index;
            public UInt64 trailing_samples;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct Status
        {
            public UInt64 sample_index;
            public float loudness;
            public float bpm;
            public float beat_phase;
            public float beat_confidence;
            public float buildup;
            public float drop;
            public UInt32 scene;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct ScoreInfo
        {
            public UInt32 sample_rate;
            public UInt32 channels;
            public UInt32 analysis_hop;
            public UInt32 reserved;
            public UInt64 duration_samples;
            public UInt64 frame_count;
            public UInt64 section_count;
            public UInt64 accent_count;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct LightPoint
        {
            public UInt32 id;
            public float x, y, z, zone;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct Rgba
        {
            public float r, g, b, a;
        }

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        public static extern UInt32 glowbeat_abi_version();

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern Config glowbeat_default_config();

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern IntPtr glowbeat_engine_create(Config config);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern void glowbeat_engine_destroy(IntPtr handle);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe Int32 glowbeat_engine_push_f32(
            IntPtr handle, float* samples, UIntPtr sampleCount, out Report report);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe Int32 glowbeat_engine_render(
            IntPtr handle, UInt64 sampleIndex, LightPoint* points,
            UIntPtr pointCount, Rgba* output);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern Int32 glowbeat_engine_status(IntPtr handle, out Status status);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe IntPtr glowbeat_score_analyze_f32(
            float* samples, UIntPtr sampleCount, UInt32 sampleRate, UInt32 channels);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe IntPtr glowbeat_score_deserialize(
            byte* bytes, UIntPtr byteCount);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern void glowbeat_score_destroy(IntPtr handle);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern Int32 glowbeat_score_info(IntPtr handle, out ScoreInfo info);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern Int32 glowbeat_score_serialized_size(
            IntPtr handle, out UIntPtr byteCount);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe Int32 glowbeat_score_serialize(
            IntPtr handle, byte* output, UIntPtr capacity, out UIntPtr written);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern IntPtr glowbeat_player_create(IntPtr score, Config config);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern void glowbeat_player_destroy(IntPtr handle);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern Int32 glowbeat_player_seek(IntPtr handle, UInt64 sampleIndex);

        [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe Int32 glowbeat_player_render(
            IntPtr handle, UInt64 sampleIndex, LightPoint* points,
            UIntPtr pointCount, Rgba* output);
    }

    public sealed class Engine : IDisposable
    {
        private IntPtr _handle;

        public Engine(uint sampleRate, uint channels, uint lightCapacity)
        {
            Native.Config config = Native.glowbeat_default_config();
            config.sample_rate = sampleRate;
            config.channels = channels;
            config.light_capacity = lightCapacity;
            _handle = Native.glowbeat_engine_create(config);
            if (_handle == IntPtr.Zero)
                throw new ArgumentException("Invalid GlowBeat streaming configuration");
        }

        public unsafe Native.Report Push(float[] interleavedPcm)
        {
            if (interleavedPcm == null) throw new ArgumentNullException(nameof(interleavedPcm));
            ThrowIfDisposed();
            fixed (float* data = interleavedPcm)
            {
                int code = Native.glowbeat_engine_push_f32(
                    _handle,
                    data,
                    new UIntPtr((uint)interleavedPcm.Length),
                    out Native.Report report);
                Check(code, "streaming PCM push");
                return report;
            }
        }

        public Native.Status GetStatus()
        {
            ThrowIfDisposed();
            int code = Native.glowbeat_engine_status(_handle, out Native.Status status);
            Check(code, "streaming status");
            return status;
        }

        public unsafe void Render(ulong sampleIndex, Native.LightPoint[] points, Native.Rgba[] output)
        {
            ValidateBuffers(points, output);
            ThrowIfDisposed();
            fixed (Native.LightPoint* pointData = points)
            fixed (Native.Rgba* outputData = output)
            {
                int code = Native.glowbeat_engine_render(
                    _handle,
                    sampleIndex,
                    pointData,
                    new UIntPtr((uint)points.Length),
                    outputData);
                Check(code, "streaming render");
            }
        }

        private void ThrowIfDisposed()
        {
            if (_handle == IntPtr.Zero) throw new ObjectDisposedException(nameof(Engine));
        }

        public void Dispose()
        {
            if (_handle != IntPtr.Zero)
            {
                Native.glowbeat_engine_destroy(_handle);
                _handle = IntPtr.Zero;
            }
            GC.SuppressFinalize(this);
        }

        ~Engine() => Dispose();

        internal static void Check(int code, string operation)
        {
            if (code != 0)
                throw new InvalidOperationException($"GlowBeat {operation} failed with status {code}");
        }

        internal static void ValidateBuffers(Native.LightPoint[] points, Native.Rgba[] output)
        {
            if (points == null) throw new ArgumentNullException(nameof(points));
            if (output == null) throw new ArgumentNullException(nameof(output));
            if (points.Length != output.Length) throw new ArgumentException("Light/output length mismatch");
        }
    }

    public sealed class Score : IDisposable
    {
        private IntPtr _handle;

        private Score(IntPtr handle)
        {
            _handle = handle;
        }

        public static unsafe Score Analyze(float[] interleavedPcm, uint sampleRate, uint channels)
        {
            if (interleavedPcm == null) throw new ArgumentNullException(nameof(interleavedPcm));
            if (channels == 0 || channels > Int32.MaxValue || interleavedPcm.Length == 0 ||
                interleavedPcm.Length % (int)channels != 0)
                throw new ArgumentException("PCM must contain complete interleaved sample frames");
            fixed (float* data = interleavedPcm)
            {
                IntPtr handle = Native.glowbeat_score_analyze_f32(
                    data,
                    new UIntPtr((uint)interleavedPcm.Length),
                    sampleRate,
                    channels);
                if (handle == IntPtr.Zero) throw new InvalidOperationException("GlowBeat full-track analysis failed");
                return new Score(handle);
            }
        }

        public static unsafe Score Deserialize(byte[] bytes)
        {
            if (bytes == null) throw new ArgumentNullException(nameof(bytes));
            if (bytes.Length == 0) throw new ArgumentException("Score data is empty", nameof(bytes));
            fixed (byte* data = bytes)
            {
                IntPtr handle = Native.glowbeat_score_deserialize(data, new UIntPtr((uint)bytes.Length));
                if (handle == IntPtr.Zero) throw new InvalidOperationException("GlowBeat score is invalid or unsupported");
                return new Score(handle);
            }
        }

        public Native.ScoreInfo Info
        {
            get
            {
                ThrowIfDisposed();
                int code = Native.glowbeat_score_info(_handle, out Native.ScoreInfo info);
                Engine.Check(code, "score info");
                return info;
            }
        }

        public unsafe byte[] Serialize()
        {
            ThrowIfDisposed();
            int code = Native.glowbeat_score_serialized_size(_handle, out UIntPtr nativeSize);
            Engine.Check(code, "score size");
            ulong size64 = nativeSize.ToUInt64();
            if (size64 > Int32.MaxValue) throw new InvalidOperationException("Score is too large for a managed byte array");
            byte[] bytes = new byte[(int)size64];
            fixed (byte* output = bytes)
            {
                code = Native.glowbeat_score_serialize(
                    _handle,
                    output,
                    new UIntPtr((uint)bytes.Length),
                    out UIntPtr written);
                Engine.Check(code, "score serialization");
                if (written.ToUInt64() != (ulong)bytes.Length)
                    throw new InvalidOperationException("GlowBeat returned an inconsistent score size");
            }
            return bytes;
        }

        public Player CreatePlayer(uint lightCapacity, Func<Native.Config, Native.Config> configure = null)
        {
            ThrowIfDisposed();
            Native.Config config = Native.glowbeat_default_config();
            config.light_capacity = lightCapacity;
            if (configure != null) config = configure(config);
            IntPtr handle = Native.glowbeat_player_create(_handle, config);
            if (handle == IntPtr.Zero) throw new ArgumentException("Invalid GlowBeat player configuration");
            return new Player(handle);
        }

        private void ThrowIfDisposed()
        {
            if (_handle == IntPtr.Zero) throw new ObjectDisposedException(nameof(Score));
        }

        public void Dispose()
        {
            if (_handle != IntPtr.Zero)
            {
                Native.glowbeat_score_destroy(_handle);
                _handle = IntPtr.Zero;
            }
            GC.SuppressFinalize(this);
        }

        ~Score() => Dispose();
    }

    public sealed class Player : IDisposable
    {
        private IntPtr _handle;

        internal Player(IntPtr handle)
        {
            _handle = handle;
        }

        public void Seek(ulong sampleIndex)
        {
            ThrowIfDisposed();
            Engine.Check(Native.glowbeat_player_seek(_handle, sampleIndex), "player seek");
        }

        public unsafe void Render(ulong sampleIndex, Native.LightPoint[] points, Native.Rgba[] output)
        {
            Engine.ValidateBuffers(points, output);
            ThrowIfDisposed();
            fixed (Native.LightPoint* pointData = points)
            fixed (Native.Rgba* outputData = output)
            {
                Engine.Check(
                    Native.glowbeat_player_render(
                        _handle,
                        sampleIndex,
                        pointData,
                        new UIntPtr((uint)points.Length),
                        outputData),
                    "full-track render");
            }
        }

        private void ThrowIfDisposed()
        {
            if (_handle == IntPtr.Zero) throw new ObjectDisposedException(nameof(Player));
        }

        public void Dispose()
        {
            if (_handle != IntPtr.Zero)
            {
                Native.glowbeat_player_destroy(_handle);
                _handle = IntPtr.Zero;
            }
            GC.SuppressFinalize(this);
        }

        ~Player() => Dispose();
    }
}
