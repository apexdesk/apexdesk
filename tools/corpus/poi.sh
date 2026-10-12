# Test data of Apache POI: odd, encrypted and damaged .doc, .xls, .docx
# and .xlsx files.
. "$CORPUS_LIB"
sparse_clone https://github.com/apache/poi "$SOURCE_DIR/poi" test-data/document test-data/spreadsheet test-data/openxml4j
