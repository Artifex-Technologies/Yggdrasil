// gpuunlock-bench: the tensor-rate check behind Settings' graphics card unlock (Labyrinth OS docs/GPU_UNLOCK.md, rule 8 and
// section 9). Written for Yggdrasil; `gpuunlock bench` compiles it with nvcc when the CUDA toolkit is installed.
//
// It measures, on one card chosen by its PCI address:
//   gemm   cuBLAS FP16 x FP16 -> FP32 accumulate (tensor cores), M = N = K = 1024 ... 8192, the median TFLOPS of each size;
//   mma    raw tensor-core MMA in registers, FP16 inputs with FP32 accumulate (the rate GeForce cards halve) and with FP16
//          accumulate (never halved: the control, which must not change when the unlock works);
//   check  one GEMM against the processor: a few thousand outputs compared with an FP32 reference from the same inputs.
// and prints one JSON object. Exit 0 when the check passed, 1 when the results were wrong, 2 on a usage or CUDA error.
//
//   gpuunlock-bench --pci 0000:01:00.0 [--quick] [--samples N]
//
// --quick runs small sizes only (seconds, a fraction of the card): to see that it builds and runs, not to measure.
// SPDX-License-Identifier: MPL-2.0

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include <cublas_v2.h>
#include <cuda_fp16.h>
#include <cuda_runtime.h>
#include <mma.h>

using namespace nvcuda;

static int fail(const char *what, const char *detail) {
    printf("{\"error\": \"%s: %s\"}\n", what, detail);
    return 2;
}

#define CUDA(x)                                                                                                               \
    do {                                                                                                                      \
        cudaError_t e_ = (x);                                                                                                 \
        if (e_ != cudaSuccess) return fail(#x, cudaGetErrorString(e_));                                                       \
    } while (0)
#define BLAS(x)                                                                                                               \
    do {                                                                                                                      \
        cublasStatus_t s_ = (x);                                                                                              \
        if (s_ != CUBLAS_STATUS_SUCCESS) return fail(#x, cublasGetStatusString(s_));                                          \
    } while (0)

// Inputs that are exact in FP16 and small, so the FP32 sum of K products has no rounding left to argue about: integers -3..3
// scaled by 1/8.
static half value(unsigned i) {
    unsigned x = i * 2654435761u;
    x ^= x >> 13;
    return __float2half((float)((int)(x % 7u) - 3) / 8.0f);
}

__global__ void fill(half *p, size_t n, unsigned seed) {
    size_t i = blockIdx.x * (size_t)blockDim.x + threadIdx.x;
    if (i < n) {
        unsigned x = (unsigned)(i + seed) * 2654435761u;
        x ^= x >> 13;
        p[i] = __float2half((float)((int)(x % 7u) - 3) / 8.0f);
    }
}

// Each warp keeps its fragments in registers and runs `iters` MMAs: the tensor cores' own rate, without memory in the way.
template <typename Acc>
__global__ void mma_loop(Acc *out, int iters) {
    wmma::fragment<wmma::matrix_a, 16, 16, 16, half, wmma::row_major> a;
    wmma::fragment<wmma::matrix_b, 16, 16, 16, half, wmma::col_major> b;
    wmma::fragment<wmma::accumulator, 16, 16, 16, Acc> c0, c1, c2, c3;
    wmma::fill_fragment(a, __float2half(0.001f));
    wmma::fill_fragment(b, __float2half(0.001f));
    wmma::fill_fragment(c0, (Acc)0);
    wmma::fill_fragment(c1, (Acc)0);
    wmma::fill_fragment(c2, (Acc)0);
    wmma::fill_fragment(c3, (Acc)0);
    for (int i = 0; i < iters; i++) {
        // four independent chains, so one MMA's result is not waited for before the next starts
        wmma::mma_sync(c0, a, b, c0);
        wmma::mma_sync(c1, a, b, c1);
        wmma::mma_sync(c2, a, b, c2);
        wmma::mma_sync(c3, a, b, c3);
    }
    for (int i = 0; i < c0.num_elements; i++) c0.x[i] = c0.x[i] + c1.x[i] + c2.x[i] + c3.x[i];
    // stored, so the compiler cannot drop the loop
    wmma::store_matrix_sync(out + (size_t)(blockIdx.x * (blockDim.x / 32) + threadIdx.x / 32) * 256, c0, 16, wmma::mem_row_major);
}

static double median(std::vector<double> v) {
    std::sort(v.begin(), v.end());
    return v[v.size() / 2];
}

template <typename Acc>
static int mma_rate(int sms, int samples, int iters, double *tflops) {
    int blocks = sms * 8, threads = 256;  // 8 warps a block
    Acc *out = nullptr;
    CUDA(cudaMalloc(&out, (size_t)blocks * (threads / 32) * 256 * sizeof(Acc)));
    mma_loop<Acc><<<blocks, threads>>>(out, 100);  // warm up
    CUDA(cudaDeviceSynchronize());
    cudaEvent_t t0, t1;
    CUDA(cudaEventCreate(&t0));
    CUDA(cudaEventCreate(&t1));
    std::vector<double> r;
    for (int s = 0; s < samples; s++) {
        CUDA(cudaEventRecord(t0));
        mma_loop<Acc><<<blocks, threads>>>(out, iters);
        CUDA(cudaEventRecord(t1));
        CUDA(cudaEventSynchronize(t1));
        float ms = 0;
        CUDA(cudaEventElapsedTime(&ms, t0, t1));
        double flops = 2.0 * 16 * 16 * 16 * 4.0 * iters * (double)blocks * (threads / 32);
        r.push_back(flops / (ms / 1e3) / 1e12);
    }
    *tflops = median(r);
    CUDA(cudaFree(out));
    return 0;
}

int main(int argc, char **argv) {
    const char *pci = nullptr;
    bool quick = false;
    int samples = 7;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--pci") && i + 1 < argc) pci = argv[++i];
        else if (!strcmp(argv[i], "--quick")) quick = true;
        else if (!strcmp(argv[i], "--samples") && i + 1 < argc) samples = std::max(1, atoi(argv[++i]));
        else return fail("usage", "gpuunlock-bench --pci DOMAIN:BUS:DEV.FN [--quick] [--samples N]");
    }
    if (!pci) return fail("usage", "--pci is required: the benchmark runs on the card named, never on a guess");
    int dev = -1;
    CUDA(cudaDeviceGetByPCIBusId(&dev, pci));
    CUDA(cudaSetDevice(dev));
    cudaDeviceProp prop;
    CUDA(cudaGetDeviceProperties(&prop, dev));
    if (quick) samples = std::min(samples, 2);

    cublasHandle_t h;
    BLAS(cublasCreate(&h));
    BLAS(cublasSetMathMode(h, CUBLAS_TENSOR_OP_MATH));

    // --- correctness: one GEMM, outputs compared with the processor's FP32 sums of the same FP16 inputs
    const int cn = 512;
    std::vector<half> ha((size_t)cn * cn), hb((size_t)cn * cn);
    for (size_t i = 0; i < ha.size(); i++) {
        ha[i] = value((unsigned)i);
        hb[i] = value((unsigned)i + 7919u);
    }
    half *da = nullptr, *db = nullptr;
    float *dc = nullptr;
    CUDA(cudaMalloc(&da, ha.size() * sizeof(half)));
    CUDA(cudaMalloc(&db, hb.size() * sizeof(half)));
    CUDA(cudaMalloc(&dc, (size_t)cn * cn * sizeof(float)));
    CUDA(cudaMemcpy(da, ha.data(), ha.size() * sizeof(half), cudaMemcpyHostToDevice));
    CUDA(cudaMemcpy(db, hb.data(), hb.size() * sizeof(half), cudaMemcpyHostToDevice));
    const float one = 1.0f, zero = 0.0f;
    // column-major: C (m x n) = A (m x k) * B (k x n)
    BLAS(cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, cn, cn, cn, &one, da, CUDA_R_16F, cn, db, CUDA_R_16F, cn, &zero, dc, CUDA_R_32F, cn,
                      CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP));
    std::vector<float> hc((size_t)cn * cn);
    CUDA(cudaMemcpy(hc.data(), dc, hc.size() * sizeof(float), cudaMemcpyDeviceToHost));
    int compared = 0, wrong = 0;
    double worst = 0;
    for (int s = 0; s < 4096; s++) {
        int row = (s * 37) % cn, col = (s * 101 + s / cn) % cn;
        double ref = 0;
        for (int k = 0; k < cn; k++) ref += (double)__half2float(ha[(size_t)k * cn + row]) * (double)__half2float(hb[(size_t)col * cn + k]);
        double got = hc[(size_t)col * cn + row];
        // every product is a multiple of 1/64 and the sums stay far below 2^24 / 64: FP32 holds them exactly
        double err = std::fabs(got - ref);
        worst = std::max(worst, err);
        if (err > 1e-6) wrong++;
        compared++;
    }
    CUDA(cudaFree(da));
    CUDA(cudaFree(db));
    CUDA(cudaFree(dc));

    // --- cuBLAS GEMM rate
    std::vector<int> sizes = quick ? std::vector<int>{1024} : std::vector<int>{1024, 2048, 4096, 8192};
    std::string gemm = "[";
    for (size_t si = 0; si < sizes.size(); si++) {
        int n = sizes[si];
        half *a = nullptr, *b = nullptr;
        float *c = nullptr;
        size_t nn = (size_t)n * n;
        CUDA(cudaMalloc(&a, nn * sizeof(half)));
        CUDA(cudaMalloc(&b, nn * sizeof(half)));
        CUDA(cudaMalloc(&c, nn * sizeof(float)));
        fill<<<(unsigned)((nn + 255) / 256), 256>>>(a, nn, 1u);
        fill<<<(unsigned)((nn + 255) / 256), 256>>>(b, nn, 2u);
        int reps = n >= 4096 ? 10 : 50;
        BLAS(cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, n, n, n, &one, a, CUDA_R_16F, n, b, CUDA_R_16F, n, &zero, c, CUDA_R_32F, n,
                          CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP));
        CUDA(cudaDeviceSynchronize());
        cudaEvent_t t0, t1;
        CUDA(cudaEventCreate(&t0));
        CUDA(cudaEventCreate(&t1));
        std::vector<double> r;
        for (int s = 0; s < samples; s++) {
            CUDA(cudaEventRecord(t0));
            for (int k = 0; k < reps; k++)
                BLAS(cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, n, n, n, &one, a, CUDA_R_16F, n, b, CUDA_R_16F, n, &zero, c, CUDA_R_32F, n,
                                  CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP));
            CUDA(cudaEventRecord(t1));
            CUDA(cudaEventSynchronize(t1));
            float ms = 0;
            CUDA(cudaEventElapsedTime(&ms, t0, t1));
            r.push_back(2.0 * n * (double)n * n * reps / (ms / 1e3) / 1e12);
        }
        char buf[96];
        snprintf(buf, sizeof buf, "%s{\"n\": %d, \"tflops\": %.2f}", si ? ", " : "", n, median(r));
        gemm += buf;
        CUDA(cudaFree(a));
        CUDA(cudaFree(b));
        CUDA(cudaFree(c));
    }
    gemm += "]";

    // --- raw MMA rates
    double f32 = 0, f16 = 0;
    int iters = quick ? 2000 : 20000;
    if (mma_rate<float>(prop.multiProcessorCount, samples, iters, &f32)) return 2;
    if (mma_rate<half>(prop.multiProcessorCount, samples, iters, &f16)) return 2;
    cublasDestroy(h);

    bool ok = wrong == 0;
    printf("{\"device\": \"%s\", \"pci\": \"%s\", \"sms\": %d, \"quick\": %s, \"samples\": %d, "
           "\"check\": {\"compared\": %d, \"wrong\": %d, \"max_abs_err\": %.3g, \"ok\": %s}, "
           "\"gemm_f16_acc_f32\": %s, \"mma_f16_acc_f32\": %.2f, \"mma_f16_acc_f16\": %.2f}\n",
           prop.name, pci, prop.multiProcessorCount, quick ? "true" : "false", samples, compared, wrong, worst, ok ? "true" : "false",
           gemm.c_str(), f32, f16);
    return ok ? 0 : 1;
}
