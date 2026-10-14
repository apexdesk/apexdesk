' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Finance
Sub Main()
    ActiveDocument.CustomDocumentProperties.Add Name:="Dept", LinkToContent:=False, Type:=msoPropertyTypeString, Value:="Finance"
    Debug.Print ActiveDocument.CustomDocumentProperties("Dept").Value
End Sub
