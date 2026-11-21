/* -*- C++ -*-
 * Copyright 2019-2025 LibRaw LLC (info@libraw.org)
 *
 LibRaw uses code from dcraw.c -- Dave Coffin's raw photo decoder,
 dcraw.c is copyright 1997-2018 by Dave Coffin, dcoffin a cybercom o net.
 LibRaw do not use RESTRICTED code from dcraw.c

 LibRaw is free software; you can redistribute it and/or modify
 it under the terms of the one of two licenses as you choose:

1. GNU LESSER GENERAL PUBLIC LICENSE version 2.1
   (See file LICENSE.LGPL provided in LibRaw distribution archive for details).

2. COMMON DEVELOPMENT AND DISTRIBUTION LICENSE (CDDL) Version 1.0
   (See file LICENSE.CDDL provided in LibRaw distribution archive for details).

 */

// Modified by the Grainy RS contributors on 2026-11-21 to adapt tile
// scheduling, CIELab conversion and homogeneity sums in LibRaw 0.22.1. This derivative
// retains the upstream license above. The original source is in
// vendor/LibRaw-0.22.1/src/demosaic/xtrans_demosaic.cpp.
#include <algorithm>
#include <memory>
#include <new>
#include <omp.h>
#include <windows.h>
#include "internal/dcraw_defs.h"

namespace {
// The upstream kernel reads the global image through offsets stored in
// signed shorts. Verify both representations of each offset. This also
// excludes unusual patterns that can wrap global reads across a row edge.
bool bounded_tile_reads(const char (&pattern)[6][6], int image_width,
                        const short (&offsets)[3][3][2][8])
{
  const int global_factors[8] = {2, 2, 1, 1, 3, 3, 0, 0};
  for (int row = 0; row < 6; row++)
    for (int col = 0; col < 6; col++)
    {
      const int y = row % 3, x = col % 3;
      const bool green = pattern[row][col] == 1;
      if (green != (pattern[y][x] == 1))
        return false;
      for (int k = 0; k < 8; k++)
      {
        const int local = offsets[y][x][1][k];
        const int dy = (local < 0 ? local - LIBRAW_AHD_TILE / 2 : local + LIBRAW_AHD_TILE / 2) / LIBRAW_AHD_TILE;
        const int dx = local - dy * LIBRAW_AHD_TILE;
        const int radius = MAX(abs(dx), abs(dy));
        if (radius > 2 || dy * image_width + dx != offsets[y][x][0][k])
          return false;
        // Non-green sites use multiples of these offsets in the global image.
        // The green refinement at positions 3..5 also needs a two-pixel local margin.
        if (!green && (radius * global_factors[k] > 3 || (k >= 3 && k <= 5 && radius > 1)))
          return false;
      }
    }
  return true;
}

const int max_tile_workers = 8;

int tile_workers(int active_tiles, size_t bytes_per_tile)
{
  const int limit = MIN(MIN(omp_get_max_threads(), active_tiles), max_tile_workers);
  if (limit <= 1)
    return 1;
  MEMORYSTATUSEX memory = {};
  memory.dwLength = sizeof(memory);
  if (!GlobalMemoryStatusEx(&memory))
    return 1;
  // Limit additional scratch buffers to an eighth of available physical
  // memory and commit space. Serial interpolation needs the first buffer.
  const unsigned long long available = MIN(memory.ullAvailPhys, memory.ullAvailPageFile);
  return static_cast<int>(MIN(static_cast<unsigned long long>(limit), 1 + available / 8 / bytes_per_tile));
}
} // namespace

#define fcol(row, col) xtrans[(row + 6) % 6][(col + 6) % 6]
/*
   Frank Markesteijn's algorithm for Fuji X-Trans sensors
 */
void LibRaw::xtrans_interpolate(int passes)
{
  int cstat[4] = {0, 0, 0, 0};
  int ndir;
  static const short orth[12] = {1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1},
                     patt[2][16] = {{0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0,
                                     0, 0, 0},
                                    {0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1,
                                     -1, -1, 1}},
                     dir[4] = {1, LIBRAW_AHD_TILE, LIBRAW_AHD_TILE + 1,
                               LIBRAW_AHD_TILE - 1};
  short allhex[3][3][2][8];
  ushort sgrow = 0, sgcol = 0;

  if (width < LIBRAW_AHD_TILE || height < LIBRAW_AHD_TILE || filters != 9)
    throw LIBRAW_EXCEPTION_IO_CORRUPT; // too small image
                                       /* Check against right pattern */
  for (int row = 0; row < 6; row++)
    for (int col = 0; col < 6; col++)
      cstat[(unsigned)fcol(row, col)]++;

  if (cstat[0] < 6 || cstat[0] > 10 || cstat[1] < 16 || cstat[1] > 24 ||
      cstat[2] < 6 || cstat[2] > 10 || cstat[3])
    throw LIBRAW_EXCEPTION_IO_CORRUPT;

  // Init allhex table to unreasonable values
  for (int i = 0; i < 3; i++)
    for (int j = 0; j < 3; j++)
      for (int k = 0; k < 2; k++)
        for (int l = 0; l < 8; l++)
          allhex[i][j][k][l] = 32700;

  cielab(0, 0);
  ndir = 4 << int(passes > 1);

  int minv = 0, maxv = 0, minh = 0, maxh = 0;
  /* Map a green hexagon around each non-green pixel and vice versa:	*/
  for (int row = 0; row < 3; row++)
    for (int col = 0; col < 3; col++)
      for (int ng = 0, d = 0; d < 10; d += 2)
      {
        int g = fcol(row, col) == 1;
        if (fcol(row + orth[d], col + orth[d + 2]) == 1)
          ng = 0;
        else
          ng++;
        if (ng == 4)
        {
          sgrow = row;
          sgcol = col;
        }
        if (ng == g + 1)
        {
            int c;
            FORC(8)
            {
                int v = orth[d] * patt[g][c * 2] + orth[d + 1] * patt[g][c * 2 + 1];
                int h = orth[d + 2] * patt[g][c * 2] + orth[d + 3] * patt[g][c * 2 + 1];
                minv = MIN(v, minv);
                maxv = MAX(v, maxv);
                minh = MIN(v, minh);
                maxh = MAX(v, maxh);
                allhex[row][col][0][c ^ (g * 2 & d)] = h + v * width;
                allhex[row][col][1][c ^ (g * 2 & d)] = h + v * LIBRAW_AHD_TILE;
            }
        }
      }

  // Check allhex table initialization
  for (int i = 0; i < 3; i++)
    for (int j = 0; j < 3; j++)
      for (int k = 0; k < 2; k++)
        for (int l = 0; l < 8; l++)
          if (allhex[i][j][k][l] > maxh + maxv * width + 1 ||
              allhex[i][j][k][l] < minh + minv * width - 1)
            throw LIBRAW_EXCEPTION_IO_CORRUPT;
  int retrycount = 0;

  /* Set green1 and green3 to the minimum and maximum allowed values:	*/
  for (int row = 2; row < height - 2; row++)
  {
      int col;
      ushort min, max;
      for (col = 2, max = 0u, min = 0xffffu; col < int(width) - 2; col++)
      {
          if (fcol(row, col) == 1 && (min = ~(max = 0)))
              continue;
          ushort(*pix)[4];
          pix = image + row * width + col;
          short* hex = allhex[row % 3][col % 3][0];
          if (!max)
          {
              int c;
              FORC(6)
              {
                  int val = pix[hex[c]][1];
                  if (min > val)
                      min = val;
                  if (max < val)
                      max = val;
              }
          }
          pix[0][1] = min;
          pix[0][3] = max;
          switch ((row - sgrow) % 3)
          {
          case 1:
              if (row < height - 3)
              {
                  row++;
                  col--;
              }
              break;
          case 2:
              if ((min = ~(max = 0)) && (col += 2) < width - 3 && row > 2)
              {
                  row--;
                  if (retrycount++ > width * height)
                      throw LIBRAW_EXCEPTION_IO_CORRUPT;
              }
          }
      }
  }

  for (int row = 3; row < 9 && row < height - 3; row++)
	  for (int col = 3; col < 9 && col < width - 3; col++)
	  {
		  if ((fcol(row, col)) == 1)
			  continue;
          short* hex = allhex[row % 3][col % 3][0];
          int c;
		  FORC(2)
		  {
			  int idx3 = 3 * hex[4 + c] + row * width + col;
			  int idx4 = -3 * hex[4 + c] + row * width + col;
			  int maxidx = width * height;
			  if (idx3 < 0 || idx3 >= maxidx)
				  throw LIBRAW_EXCEPTION_IO_CORRUPT;
			  if (idx4 < 0 || idx4 >= maxidx)
				  throw LIBRAW_EXCEPTION_IO_CORRUPT;
		  }
	  }


  const int stride = LIBRAW_AHD_TILE - 16;
  const int rows = 1 + (int(height) - 23) / stride;
  const int cols = 1 + (int(width) - 23) / stride;
  const size_t buffer_size = size_t(LIBRAW_AHD_TILE) * LIBRAW_AHD_TILE * (ndir * 11 + 6);
  const int capacity = bounded_tile_reads(xtrans, width, allhex) ? MIN(rows, (cols + 1) / 2) : 1;
  const int wanted = tile_workers(capacity, buffer_size);
  std::unique_ptr<char[]> buffers[max_tile_workers];
  int workers = 0;
  while (workers < wanted)
  {
    buffers[workers].reset(new(std::nothrow) char[buffer_size]());
    if (!buffers[workers])
      break;
    workers++;
  }
  if (!workers)
    throw std::bad_alloc();

  auto interpolate_tile = [&](int top, int left, char *buffer) {
    ushort(*rgb)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE][3], (*rix)[3];
    short(*lab)[LIBRAW_AHD_TILE][3], (*lix)[3];
    float(*drv)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE];
    char(*homo)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE];

    rgb = (ushort(*)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE][3])buffer;
    lab = (short(*)[LIBRAW_AHD_TILE][3])(
        buffer + LIBRAW_AHD_TILE * LIBRAW_AHD_TILE * (ndir * 6));
    drv = (float(*)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE])(
        buffer + LIBRAW_AHD_TILE * LIBRAW_AHD_TILE * (ndir * 6 + 6));
    homo = (char(*)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE])(
        buffer + LIBRAW_AHD_TILE * LIBRAW_AHD_TILE * (ndir * 10 + 6));

    int mrow = MIN(top + LIBRAW_AHD_TILE, height - 3);
    int mcol = MIN(left + LIBRAW_AHD_TILE, width - 3);
    for (int row = top; row < mrow; row++)
        for (int col = left; col < mcol; col++)
            memcpy(rgb[0][row - top][col - left], image[row * width + col], 6);
    int c;
    FORC3 memcpy(rgb[c + 1], rgb[0], sizeof * rgb);

    /* Interpolate green horizontally, vertically, and along both diagonals:
     */
    int color[3][8];
    for (int row = top; row < mrow; row++)
        for (int col = left; col < mcol; col++)
        {
            int f;
            if ((f = fcol(row, col)) == 1)
                continue;
            ushort (*pix)[4] = image + row * width + col;
            short* hex = allhex[row % 3][col % 3][0];
            color[1][0] = 174 * (pix[hex[1]][1] + pix[hex[0]][1]) -
                46 * (pix[2 * hex[1]][1] + pix[2 * hex[0]][1]);
            color[1][1] = 223 * pix[hex[3]][1] + pix[hex[2]][1] * 33 +
                92 * (pix[0][f] - pix[-hex[2]][f]);
            FORC(2)
                color[1][2 + c] = 164 * pix[hex[4 + c]][1] +
                92 * pix[-2 * hex[4 + c]][1] +
                33 * (2 * pix[0][f] - pix[3 * hex[4 + c]][f] -
                    pix[-3 * hex[4 + c]][f]);
            FORC4 rgb[c ^ !((row - sgrow) % 3)][row - top][col - left][1] =
                LIM(color[1][c] >> 8, pix[0][1], pix[0][3]);
        }

    for (int pass = 0; pass < passes; pass++)
    {
        if (pass == 1)
            memcpy(rgb += 4, buffer, 4 * sizeof * rgb);

        /* Recalculate green from interpolated values of closer pixels:	*/
        if (pass)
        {
            for (int row = top + 2; row < mrow - 2; row++)
                for (int col = left + 2; col < mcol - 2; col++)
                {
                    int f;
                    if ((f = fcol(row, col)) == 1)
                        continue;
                    ushort(*pix)[4] = image + row * width + col;
                    short* hex = allhex[row % 3][col % 3][1];
                    for (int d = 3; d < 6; d++)
                    {
                        rix =
                            &rgb[(d - 2) ^ !((row - sgrow) % 3)][row - top][col - left];
                        int val = rix[-2 * hex[d]][1] + 2 * rix[hex[d]][1] -
                            rix[-2 * hex[d]][f] - 2 * rix[hex[d]][f] + 3 * rix[0][f];
                        rix[0][1] = LIM(val / 3, pix[0][1], pix[0][3]);
                    }
                }
        }

        /* Interpolate red and blue values for solitary green pixels:	*/
        for (int row = (top - sgrow + 4) / 3 * 3 + sgrow; row < mrow - 2; row += 3)
            for (int col = (left - sgcol + 4) / 3 * 3 + sgcol; col < mcol - 2; col += 3)
        {
            rix = &rgb[0][row - top][col - left];
            int h = fcol(row, col + 1);

					if (h == 1) // Incorrect pattern
              break;

            float diff[6];
            memset(diff, 0, sizeof diff);
            for (int i = 1, d = 0; d < 6; d++, i ^= LIBRAW_AHD_TILE ^ 1, h ^= 2)
            {
                for (c = 0; c < 2; c++, h ^= 2)
                {
                    int g = 2 * rix[0][1] - rix[i << c][1] - rix[-i << c][1];
                    color[h][d] = g + rix[i << c][h] + rix[-i << c][h];
                    if (d > 1)
                        diff[d] += SQR((float)rix[i << c][1] - (float)rix[-i << c][1] -
                            (float)rix[i << c][h] + (float)rix[-i << c][h]) + SQR((float)g);
                }
                if (d > 1 && (d & 1))
                    if (diff[d - 1] < diff[d])
                        FORC(2) color[c * 2][d] = color[c * 2][d - 1];
                if (d < 2 || (d & 1))
                {
                    FORC(2) rix[0][c * 2] = CLIP(color[c * 2][d] / 2);
                    rix += LIBRAW_AHD_TILE * LIBRAW_AHD_TILE;
                }
            }
        }

        /* Interpolate red for blue pixels and vice versa:		*/
        for (int row = top + 3; row < mrow - 3; row++)
            for (int col = left + 3; col < mcol - 3; col++)
            {
                int f;
                if ((f = 2 - fcol(row, col)) == 1)
                    continue;
                rix = &rgb[0][row - top][col - left];
                c = (row - sgrow) % 3 ? LIBRAW_AHD_TILE : 1;
                int h = 3 * (c ^ LIBRAW_AHD_TILE ^ 1);
                for (int d = 0; d < 4; d++, rix += LIBRAW_AHD_TILE * LIBRAW_AHD_TILE)
                {
                    int i = d > 1 || ((d ^ c) & 1) ||
                        ((ABS(rix[0][1] - rix[c][1]) +
                            ABS(rix[0][1] - rix[-c][1])) <
                            2 * (ABS(rix[0][1] - rix[h][1]) +
                                ABS(rix[0][1] - rix[-h][1])))
                        ? c
                        : h;
                    rix[0][f] = CLIP((rix[i][f] + rix[-i][f] + 2 * rix[0][1] -
                        rix[i][1] - rix[-i][1]) /
                        2);
                }
            }

        /* Fill in red and blue for 2x2 blocks of green:		*/
        for (int row = top + 2; row < mrow - 2; row++)
            if ((row - sgrow) % 3)
                for (int col = left + 2; col < mcol - 2; col++)
                    if ((col - sgcol) % 3)
                    {
                        rix = &rgb[0][row - top][col - left];
                        short* hex = allhex[row % 3][col % 3][1];
                        for (int d = 0; d < 8;
                            d += 2, rix += LIBRAW_AHD_TILE * LIBRAW_AHD_TILE)
                            if (hex[d] + hex[d + 1])
                            {
                                int g = 3 * rix[0][1] - 2 * rix[hex[d]][1] - rix[hex[d + 1]][1];
                                for (c = 0; c < 4; c += 2)
                                    rix[0][c] = CLIP(
                                        (g + 2 * rix[hex[d]][c] + rix[hex[d + 1]][c]) / 3);
                            }
                            else
                            {
                                int g = 2 * rix[0][1] - rix[hex[d]][1] - rix[hex[d + 1]][1];
                                for (c = 0; c < 4; c += 2)
                                    rix[0][c] =
                                    CLIP((g + rix[hex[d]][c] + rix[hex[d + 1]][c]) / 2);
                            }
                    }
    }
    rgb = (ushort(*)[LIBRAW_AHD_TILE][LIBRAW_AHD_TILE][3])buffer;
    mrow -= top;
    mcol -= left;

    /* Convert to CIELab and differentiate in all directions:	*/
    // no effect
    for (int d = 0; d < ndir; d++)
    {
        for (int row = 2; row < mrow - 2; row++)
        {
#ifndef LIBRAW_NOTHREADS
            if (colors == 3)
            {
                const auto &lab_matrix = tls->ahd_data.xyz_cam;
                const float *lab_curve = tls->ahd_data.cbrt;
                for (int col = 2; col < mcol - 2; col++)
                {
                    const ushort *pixel = rgb[d][row][col];
                    float xyz[3];
                    for (int axis = 0; axis < 3; axis++)
                    {
                        float value = 0.5f;
                        value += lab_matrix[axis][0] * pixel[0];
                        value += lab_matrix[axis][1] * pixel[1];
                        value += lab_matrix[axis][2] * pixel[2];
                        xyz[axis] = lab_curve[CLIP(int(value))];
                    }
                    lab[row][col][0] = short(64 * (116 * xyz[1] - 16));
                    lab[row][col][1] = short(64 * 500 * (xyz[0] - xyz[1]));
                    lab[row][col][2] = short(64 * 200 * (xyz[1] - xyz[2]));
                }
                continue;
            }
#endif
            for (int col = 2; col < mcol - 2; col++)
                cielab(rgb[d][row][col], lab[row][col]);
        }
        for (int f = dir[d & 3], row = 3; row < mrow - 3; row++)
            for (int col = 3; col < mcol - 3; col++)
            {
                lix = &lab[row][col];
                int g = 2 * lix[0][0] - lix[f][0] - lix[-f][0];
                drv[d][row][col] =
							float(
                    SQR(g) +
                    SQR((2 * lix[0][1] - lix[f][1] - lix[-f][1] + g * 500 / 232)) +
                    SQR((2 * lix[0][2] - lix[f][2] - lix[-f][2] - g * 500 / 580))
								);
            }
    }

    /* Build homogeneity maps from the derivatives:			*/
    memset(homo, 0, ndir * LIBRAW_AHD_TILE * LIBRAW_AHD_TILE);
    for (int row = 4; row < mrow - 4; row++) {
        // Process one direction's contiguous image row at a time. Each
        // threshold is shared by all directions and all nine neighbours.
        float row_limit[LIBRAW_AHD_TILE];
        std::fill_n(row_limit + 4, std::max(0, mcol - 8), FLT_MAX);
        for (int d = 0; d < ndir; d++)
            for (int col = 4; col < mcol - 4; col++)
                if (row_limit[col] > drv[d][row][col])
                    row_limit[col] = drv[d][row][col];
        for (int col = 4; col < mcol - 4; col++)
            row_limit[col] *= 8;
        for (int d = 0; d < ndir; d++) {
            const float *above = drv[d][row - 1], *at = drv[d][row], *below = drv[d][row + 1];
            for (int col = 4; col < mcol - 4; col++) {
                const float t = row_limit[col];
                homo[d][row][col] = char((above[col-1] <= t) + (above[col] <= t) + (above[col+1] <= t)
                                       + (at[col-1] <= t) + (at[col] <= t) + (at[col+1] <= t)
                                       + (below[col-1] <= t) + (below[col] <= t) + (below[col+1] <= t));
            }
        }
    }

    /* Average the most homogeneous pixels for the final result:	*/
    if (height - top < LIBRAW_AHD_TILE + 4)
        mrow = height - top + 2;
    if (width - left < LIBRAW_AHD_TILE + 4)
        mcol = width - left + 2;
    const int first_col = MIN(left, 8);
    for (int row = MIN(top, 8); row < mrow - 8; row++)
    {
        // A map entry is at most 9. Both the five-row sums (45) and the
        // complete 5x5 windows (225) fit in an unsigned byte.
        unsigned char windows[8][LIBRAW_AHD_TILE];
        for (int d = 0; d < ndir; d++)
        {
            unsigned char vertical[LIBRAW_AHD_TILE];
            for (int x = first_col - 2; x < mcol - 6; x++)
                vertical[x] = homo[d][row-2][x] + homo[d][row-1][x] + homo[d][row][x]
                            + homo[d][row+1][x] + homo[d][row+2][x];
            for (int x = first_col; x < mcol - 8; x++)
                windows[d][x] = vertical[x-2] + vertical[x-1] + vertical[x] + vertical[x+1] + vertical[x+2];
        }
        for (int col = first_col; col < mcol - 8; col++)
        {
            int hm[8];
            for (int d = 0; d < ndir; d++)
                hm[d] = windows[d][col];
            for (int d = 0; d < ndir - 4; d++)
                if (hm[d] < hm[d + 4])
                    hm[d] = 0;
                else if (hm[d] > hm[d + 4])
                    hm[d + 4] = 0;
            ushort max;
            int d;
            for (d = 1, max = hm[0]; d < ndir; d++)
                if (max < hm[d])
                    max = hm[d];
            max -= max >> 3;

            int avg[4];
            memset(avg, 0, sizeof avg);
            for (int dd = 0; dd < ndir; dd++)
                if (hm[dd] >= max)
                {
                    FORC3 avg[c] += rgb[dd][row][col][c];
                    avg[3]++;
                }
            FORC3 image[(row + top) * width + col + left][c] = avg[c] / avg[3];
        }
    }

  };
  if (workers == 1) {
    for (int row = 0; row < rows; row++)
      for (int col = 0; col < cols; col++)
        interpolate_tile(3 + row * stride, 3 + col * stride, buffers[0].get());
  } else {
    // Read/write rectangles can overlap only between neighbouring tiles.
    // Level 2*row+col follows the raster order for left, above-left, above
    // and above-right neighbours. The implicit omp-for barrier finishes each
    // level before its dependants run. Every worker has its own tile scratch.
    const int levels = 2 * (rows - 1) + cols;
    #pragma omp parallel num_threads(workers)
    {
      char *buffer = buffers[omp_get_thread_num()].get();
      for (int level = 0; level < levels; level++) {
        const int first = MAX(0, (level - cols + 2) / 2);
        const int last = MIN(rows - 1, level / 2);
        #pragma omp for schedule(static)
        for (int row = first; row <= last; row++) {
          const int col = level - 2 * row;
          interpolate_tile(3 + row * stride, 3 + col * stride, buffer);
        }
      }
    }
  }

  border_interpolate(8);
}
#undef fcol
