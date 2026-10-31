// Runs Clamp/ClampDSP.h of clamp-mac on a binary script and writes the double output.
//
// Usage: clamp_ref <script> <output>
//
// Script records (little-endian):
//   'n' f64 sample_rate, u32 max_block           new ClampDSP instance
//   'p' u8 bypass, u8 oversampling, u8 type, u8 mode, f64 in, out, ceiling, shape
//   'b' u32 n, n f64 left, n f64 right           process one block
// Each block writes n f64 left, n f64 right, then f32 post, f32 pre (the last meter window).
//
// Like the plugin's OnParamChange, a change of OVERSAMPLING requests the oversampler reset.

#include "ClampDSP.h"

#include <cstdint>
#include <cstdio>
#include <memory>
#include <vector>

template <typename T>
static bool Read(FILE* f, T& v)
{
  return fread(&v, sizeof(T), 1, f) == 1;
}

int main(int argc, char** argv)
{
  if (argc != 3)
    return 2;
  FILE* in = fopen(argv[1], "rb");
  FILE* out = fopen(argv[2], "wb");
  if (!in || !out)
    return 2;

  std::unique_ptr<ClampDSP> dsp;
  ClampDSP::Params p;
  float meter[2] = {0.f, 0.f};
  auto send = [&meter](float post, float pre) {
    meter[0] = post;
    meter[1] = pre;
  };
  std::vector<double> l, r, ol, orr;

  char tag;
  while (Read(in, tag))
  {
    if (tag == 'n')
    {
      double sr;
      uint32_t maxBlock;
      Read(in, sr);
      Read(in, maxBlock);
      dsp = std::make_unique<ClampDSP>();
      dsp->SetSampleRate(sr);
      dsp->SetBlockSize(static_cast<int>(maxBlock));
      p = ClampDSP::Params();
      meter[0] = meter[1] = 0.f;
    }
    else if (tag == 'p')
    {
      uint8_t b[4];
      fread(b, 1, 4, in);
      const int oversampling = b[1];
      if (oversampling != p.oversampling)
        dsp->RequestOversamplingReset();
      p.bypass = b[0] != 0;
      p.oversampling = oversampling;
      p.type = b[2];
      p.mode = b[3];
      Read(in, p.inDB);
      Read(in, p.outDB);
      Read(in, p.ceilingDB);
      Read(in, p.shape);
    }
    else if (tag == 'b')
    {
      uint32_t n;
      Read(in, n);
      l.resize(n);
      r.resize(n);
      ol.resize(n);
      orr.resize(n);
      fread(l.data(), sizeof(double), n, in);
      fread(r.data(), sizeof(double), n, in);
      double* ins[2] = {l.data(), r.data()};
      double* outs[2] = {ol.data(), orr.data()};
      dsp->Process(ins, outs, static_cast<int>(n), p, send);
      fwrite(ol.data(), sizeof(double), n, out);
      fwrite(orr.data(), sizeof(double), n, out);
      fwrite(meter, sizeof(float), 2, out);
    }
    else
    {
      return 3;
    }
  }
  fclose(out);
  return 0;
}
