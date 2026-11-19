/* Test-fixture generator only: arithmetic transcode without changing coefficients. */
#include <stdio.h>
#include <stdlib.h>
#include "grainy_jpeg_symbols.h"
#include "jpeglib.h"
int main(int argc, char **argv) {
    struct jpeg_decompress_struct input;
    struct jpeg_compress_struct output;
    struct jpeg_error_mgr in_error, out_error;
    jvirt_barray_ptr *coefficients;
    FILE *source, *destination;
    if (argc != 4) return 2;
    source = fopen(argv[1], "rb");
    destination = fopen(argv[2], "wb");
    if (!source || !destination) return 3;
    input.err = jpeg_std_error(&in_error);
    output.err = jpeg_std_error(&out_error);
    jpeg_create_decompress(&input);
    jpeg_create_compress(&output);
    jpeg_stdio_src(&input, source);
    jpeg_stdio_dest(&output, destination);
    jpeg_read_header(&input, TRUE);
    coefficients = jpeg_read_coefficients(&input);
    jpeg_copy_critical_parameters(&input, &output);
    output.arith_code = TRUE;
    if (atoi(argv[3])) jpeg_simple_progression(&output);
    output.restart_interval = 2;
    jpeg_write_coefficients(&output, coefficients);
    jpeg_finish_compress(&output);
    jpeg_finish_decompress(&input);
    jpeg_destroy_compress(&output);
    jpeg_destroy_decompress(&input);
    fclose(destination);
    fclose(source);
    return 0;
}
