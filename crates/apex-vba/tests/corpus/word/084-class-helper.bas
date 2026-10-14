' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
'--- module Tally class
Private n As Long
Public Sub Add(ByVal k As Long)
    n = n + k
End Sub
Public Property Get Total() As Long
    Total = n
End Property
'--- module Module1 standard
Sub Main()
    Dim t As New Tally, p As Paragraph
    For Each p In ActiveDocument.Paragraphs
        t.Add 1
    Next p
    Debug.Print t.Total
End Sub
