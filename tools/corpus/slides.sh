# Presentations (block 7): the test files of LibreOffice Impress (sd/qa),
# Apache POI (test-data/slideshow), python-pptx and the ODF Toolkit, and
# Microsoft's Open XML SDK. Kept apart from the other sources, whose
# clones only check out documents and spreadsheets.
. "$CORPUS_LIB"
CORPUS_EXTS="$SLIDES_EXTS"
sparse_clone https://git.libreoffice.org/core "$SOURCE_DIR/libreoffice" sd/qa oox/qa
sparse_clone https://github.com/apache/poi "$SOURCE_DIR/poi" test-data/slideshow
sparse_clone https://github.com/scanny/python-pptx "$SOURCE_DIR/python-pptx" ""
sparse_clone https://github.com/tdf/odftoolkit "$SOURCE_DIR/odftoolkit" ""
sparse_clone https://github.com/dotnet/Open-XML-SDK "$SOURCE_DIR/open-xml-sdk" ""
